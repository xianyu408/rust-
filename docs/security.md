# Security model

EDA tools execute generated code and should be treated as untrusted workload execution.

## Phase-1 safeguards

- Artifact paths must be relative.
- Parent directory traversal is rejected.
- Commands are spawned without a shell.
- Commands run with fixed argument vectors.
- Commands have a default timeout.

## Required before multi-user deployment

- Run jobs in containers or a locked worker account.
- Use per-job temporary directories.
- Enforce CPU, memory, wall-clock, and output-size limits.
- Deny network from simulator workers unless explicitly needed.
- Persist audit records for generated files and commands.
- Scan generated testbenches for `$system`, DPI, VPI, and file I/O before execution.

## Command policy

Never pass agent output into a shell command string. Convert design artifacts into files, validate paths, then call known EDA binaries with fixed argument lists.

