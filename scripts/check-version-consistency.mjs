import { readFileSync } from 'node:fs';

const packageJson = JSON.parse(readFileSync('package.json', 'utf8'));
const expectedVersion = packageJson.version;
const errors = [];

function checkRegex(file, pattern, label) {
  const content = readFileSync(file, 'utf8');
  const match = content.match(pattern);
  if (!match) {
    errors.push(`${label}: version not found in ${file}`);
    return;
  }
  if (match[1] !== expectedVersion) {
    errors.push(`${label}: expected ${expectedVersion}, found ${match[1]} in ${file}`);
  }
}

checkRegex('Cargo.toml', /\[workspace\.package\][\s\S]*?version\s*=\s*"([^"]+)"/, 'workspace Cargo.toml');
checkRegex('src-tauri/Cargo.toml', /\[package\][\s\S]*?version\s*=\s*"([^"]+)"/, 'src-tauri Cargo.toml');
checkRegex('src-tauri/tauri.conf.json', /"version"\s*:\s*"([^"]+)"/, 'tauri.conf.json');

const refName = process.env.GITHUB_REF_NAME;
if (refName?.startsWith('v') && refName.slice(1) !== expectedVersion) {
  errors.push(`git tag ${refName} does not match package.json version ${expectedVersion}`);
}

const releaseVersionPattern = /(?<![\d.])\d+\.\d+\.\d+(?![\d.])/g;
for (const file of ['README.md', 'BUILD.md', 'RELEASE_NOTES.md', 'WINDOWS_TESTING.md']) {
  const content = readFileSync(file, 'utf8');
  const versions = new Set(content.match(releaseVersionPattern) ?? []);
  for (const version of versions) {
    if (version !== expectedVersion) {
      errors.push(`${file}: expected release version ${expectedVersion}, found ${version}`);
    }
  }
}

if (errors.length > 0) {
  console.error('Release version consistency check failed:');
  for (const error of errors) {
    console.error(`- ${error}`);
  }
  process.exit(1);
}

console.log(`Release version references are consistent: ${expectedVersion}`);
