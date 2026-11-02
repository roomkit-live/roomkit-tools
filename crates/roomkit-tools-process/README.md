# Process tools

## Host observation

Subscribe with `ProcessSession::subscribe()` before calling `observe()`. The watch
receiver coalesces starts, completions, removal and shutdown; output chunks do not
produce notifications. Observations include the exact command, trusted owner key,
elapsed runtime (frozen on completion) and current status. This is a privileged
host API: filter owners before displaying jobs to a user. It does not change the
owner-scoped model tool contracts. Use bounded `read()` pages when a view is open,
and drop the watch receiver when the view disconnects.
