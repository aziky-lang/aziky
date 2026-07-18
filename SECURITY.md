# Security Policy

## Supported versions

Aziky is pre-1.0 alpha software. Security fixes are made on the latest `main`
branch only. No released compiler, language ABI, or standard-library version is
currently covered by a long-term support promise.

## Reporting a vulnerability

Do not report a suspected vulnerability in a public issue.

Use GitHub’s **Security → Report a vulnerability** flow after the repository is
published and private vulnerability reporting is enabled. If that facility is
not available, contact Yassine Azily privately at
`yassine0171550@gmail.com`.

Include:

- affected commit or version;
- target and host environment;
- minimal reproduction;
- expected and actual behavior;
- security impact and attack assumptions;
- whether the report or exploit has been shared elsewhere.

Maintainers will acknowledge receipt when possible, investigate the report,
coordinate a fix and disclosure, and credit reporters who want attribution.
Because the project is currently volunteer-maintained, no fixed response-time
service level is promised.

Compiler crashes on untrusted source, incorrect ownership enforcement,
miscompilation, unsafe generated-code behavior, package checksum bypasses, and
path traversal in package/tooling operations are all in scope.
