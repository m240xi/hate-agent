# Security Policy

## Reporting a vulnerability

Please report security issues privately via GitHub Security Advisories
("Report a vulnerability" on the repository's Security tab). Do not open
public issues for security reports.

You can expect an acknowledgement within a few days. Please include:

- what the issue is and where it lives (file, command, endpoint)
- reproduction steps or a proof of concept
- the impact you believe it has

## Scope notes

- hate-agent executes shell commands and edits files on your machine. The
  path jail, command classifier, and permission prompts are the security
  boundary — bugs that escape them are in scope.
- Provider credentials live in the OS keychain; anything that leaks them to
  logs, the transcript, or the network is in scope.
- The benchmark harness and dev tooling are lower severity than the shipped
  agent core.
