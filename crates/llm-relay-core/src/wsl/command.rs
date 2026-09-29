//! Bound WSL execution and pipe I/O so a hung subsystem cannot hold the switch lock.
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::process::{Command, Output, Stdio};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_DELAY: Duration = Duration::from_secs(30);
static TIMEOUTS: OnceLock<Mutex<HashMap<Option<String>, Instant>>> = OnceLock::new();

pub(super) fn run(args: &[&str], input: Option<&[u8]>) -> io::Result<Output> {
    use std::os::windows::process::CommandExt;
    // A discovery timeout affects the whole subsystem, not just one distro.
    let distro = if args.first() == Some(&"-d") {
        args.get(1).map(|name| name.to_string())
    } else {
        None
    };
    let timeouts = TIMEOUTS.get_or_init(Default::default);
    {
        let mut recent = timeouts.lock().unwrap();
        recent.retain(|_, at| at.elapsed() < RETRY_DELAY);
        if recent.contains_key(&None) || recent.contains_key(&distro) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "WSL recently timed out; sync will retry after a short cooldown",
            ));
        }
    }
    let mut command = Command::new("wsl.exe");
    command.args(args).creation_flags(0x0800_0000);
    let result = output_with_timeout(&mut command, input, TIMEOUT);
    if result
        .as_ref()
        .is_err_and(|e| e.kind() == io::ErrorKind::TimedOut)
    {
        timeouts.lock().unwrap().insert(distro, Instant::now());
    }
    result
}

fn output_with_timeout(
    command: &mut Command,
    input: Option<&[u8]>,
    timeout: Duration,
) -> io::Result<Output> {
    let start = Instant::now();
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let stdin = child.stdin.take();
    let input = input.map(Vec::from);
    let (tx, rx) = mpsc::channel();
    // Drain outputs and write stdin concurrently to avoid full-pipe deadlocks.
    for (index, mut pipe) in [
        (0, Box::new(stdout) as Box<dyn Read + Send>),
        (1, Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = pipe.read_to_end(&mut bytes).map(|_| bytes);
            let _ = tx.send((index, result));
        });
    }
    std::thread::spawn(move || {
        let result = match (stdin, input) {
            (Some(mut stdin), Some(bytes)) => stdin.write_all(&bytes).map(|_| Vec::new()),
            _ => Ok(Vec::new()),
        };
        let _ = tx.send((2, result));
    });
    let result = (|| {
        let mut pipes = [None, None, None];
        loop {
            while let Ok((index, result)) = rx.try_recv() {
                pipes[index] = Some(result?);
            }
            if let Some(status) = child.try_wait()? {
                if pipes.iter().all(Option::is_some) {
                    return Ok(Output {
                        status,
                        stdout: pipes[0].take().unwrap(),
                        stderr: pipes[1].take().unwrap(),
                    });
                }
            }
            if start.elapsed() >= timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("WSL command timed out after {}s", timeout.as_secs_f64()),
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        // Do not join workers: inherited handles must not extend the deadline.
        let _ = child.try_wait();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args([
            "--exact",
            "wsl::command::tests::subprocess",
            "--ignored",
            "--nocapture",
        ]);
        command.env("LLM_RELAY_WSL_PROCESS_TEST", mode);
        command
    }

    #[test]
    #[ignore = "helper subprocess for timeout tests"]
    fn subprocess() {
        match std::env::var("LLM_RELAY_WSL_PROCESS_TEST")
            .unwrap()
            .as_str()
        {
            "hang" => std::thread::sleep(Duration::from_secs(60)),
            "pipes" => {
                let bytes = vec![b'x'; 256 * 1024];
                io::stdout().write_all(&bytes).unwrap();
                io::stderr().write_all(&bytes).unwrap();
                let mut input = Vec::new();
                io::stdin().read_to_end(&mut input).unwrap();
                assert_eq!(input, bytes);
            }
            "failure" => std::process::exit(7),
            _ => unreachable!(),
        }
    }

    #[test]
    fn hung_process_and_blocked_stdin_are_bounded() {
        for input in [None, Some(vec![b'x'; 1024 * 1024])] {
            let start = Instant::now();
            let error = output_with_timeout(
                &mut child("hang"),
                input.as_deref(),
                Duration::from_millis(250),
            )
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            assert!(start.elapsed() < Duration::from_secs(3));
        }
    }

    #[test]
    fn drains_large_pipes_while_writing_stdin() {
        let bytes = vec![b'x'; 256 * 1024];
        let output =
            output_with_timeout(&mut child("pipes"), Some(&bytes), Duration::from_secs(5)).unwrap();
        assert!(output.status.success());
        assert!(output.stdout.len() >= bytes.len());
        assert_eq!(output.stderr, bytes);
    }

    #[test]
    fn preserves_nonzero_exit_status() {
        let output =
            output_with_timeout(&mut child("failure"), None, Duration::from_secs(5)).unwrap();
        assert_eq!(output.status.code(), Some(7));
    }
}
