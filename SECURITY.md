# Security policy

## Supported versions

RoomKit Tools is in early development. Security fixes land on the `main`
branch and in the next preview; there are no maintained release branches yet.

## Reporting a vulnerability

Please do not report security issues in public issues, discussions or pull
requests. Report them privately through GitHub's
[private vulnerability reporting](https://github.com/roomkit-live/roomkit-tools/security/advisories/new).

Include what you can of the following:

- the affected tool, crate or package, and the version or commit;
- a description of the issue and its impact;
- the tool arguments, files or steps, or a minimal program, that reproduce it.

We will acknowledge the report, keep you informed while we work on a fix, and
credit you in the advisory unless you prefer otherwise.

## Scope

The model that chooses tool arguments is treated as untrusted, and so are the
files, web pages, search results and command output it reads. Reports of
particular interest include:

- a filesystem, outline or image tool reading or changing a path outside the
  host's workspace root;
- `web_fetch` reaching a private, loopback or link-local address, or an origin
  the host did not allow;
- an API key sent to a host other than the configured service, or appearing in
  an error, a log or a result;
- one conversation or agent reading, writing or stopping another one's
  processes, output archives, plans or tasks when the host configured them
  separately;
- a panic, or memory, CPU or disk use beyond the documented limits, driven by
  tool arguments or by the content a tool reads;
- a process that survives the timeout, cancellation or cleanup that should
  have stopped it.

Some capabilities are intentionally broad and are not vulnerabilities by
themselves: `shell` and the process tools run commands with the host's
permissions and environment, and the filesystem checks are not an
operating-system sandbox. Choosing which tools an agent gets, and isolating the
host itself, is the host application's responsibility.
