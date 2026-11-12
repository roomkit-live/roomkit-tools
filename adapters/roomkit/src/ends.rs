//! A background job's end, told to the agent that started it.

use std::pin::Pin;
use std::time::Duration;

use futures_core::Stream;
use roomkit::model::{ChannelId, RoomId};
use roomkit::{ConversationStore, DeliverOptions, RoomKit, RoomKitError};
use roomkit_tools_process::{JobEnd, JobEnds, JobTool, ProcessEnd};

/// The longest command a notice quotes, in characters.
const COMMAND_CHARS: usize = 200;

/// Tell each RoomKit agent when a background job it started ends:
/// Python's `notify_job_ends(kit, ends)`.
///
/// `ends` is a [`ProcessSession`](roomkit_tools_process::ProcessSession)'s
/// or a [`ShellTool`](roomkit_tools_process::ShellTool)'s, taken before the
/// agents start their jobs; spawn one call per session. Each end of a job the
/// process or shell adapters started reaches its agent as an instruction
/// (`deliver()` with `instruction`, addressed to it), which gives the agent a
/// turn, or is read by the turn it is running. A delivery lasts the turn it
/// gives, so each runs in a task of its own: one agent's turn never holds back
/// another's notice. A job stopped, by the agent, the host or a close, is not
/// told: whoever stopped it knows. An end whose owner is not a RoomKit room
/// and agent is skipped; one the kit refuses is logged, unless it closed.
/// Returns once `ends` stopped and every delivery begun has ended.
pub async fn notify_job_ends<S>(kit: &RoomKit<S>, mut ends: JobEnds)
where
    S: ConversationStore + Send + Sync + 'static,
{
    let mut delivering = tokio::task::JoinSet::new();
    while let Some(end) = next(&mut ends).await {
        while delivering.try_join_next().is_some() {}
        let (Some((room, agent)), Some(notice)) = (caller(end.owner.as_deref()), notice(&end))
        else {
            continue;
        };
        delivering.spawn(tell(kit.clone(), room, agent, notice));
    }
    while delivering.join_next().await.is_some() {}
}

/// Deliver `notice` to `agent` in `room`, as the application's instruction.
async fn tell<S: ConversationStore>(
    kit: RoomKit<S>,
    room: RoomId,
    agent: ChannelId,
    notice: String,
) {
    let options = DeliverOptions {
        addressed_to: Some(vec![agent]),
        instruction: true,
        ..DeliverOptions::default()
    };
    match kit.deliver(&room, notice, options).await {
        Ok(_) | Err(RoomKitError::KitClosed) => {}
        Err(error) => tracing::warn!(
            target: "roomkit_tools",
            "the end of a background job was not told in room {room}: {error}"
        ),
    }
}

async fn next(ends: &mut JobEnds) -> Option<JobEnd> {
    std::future::poll_fn(|context| Pin::new(&mut *ends).poll_next(context)).await
}

/// The room and agent an owner key names: [`roomkit_owner`](crate::roomkit_owner)'s.
fn caller(owner: Option<&str>) -> Option<(RoomId, ChannelId)> {
    let (room, agent) = serde_json::from_str::<(String, String)>(owner?).ok()?;
    Some((RoomId::new(room).ok()?, ChannelId::new(agent).ok()?))
}

/// What the agent reads of `end`; `None` for a stop, which it is not told.
fn notice(end: &JobEnd) -> Option<String> {
    let command = quoted(&end.command);
    let after = duration(end.elapsed_ms);
    let process = || {
        let id = end.process_id.as_deref().unwrap_or("?");
        format!("Background process {id} ({command})")
    };
    Some(match (end.tool, end.end) {
        (_, ProcessEnd::Stopped) => return None,
        (JobTool::Shell, _) => format!(
            "The background jobs of the shell command {command} ended after {after}. \
             Their exit status is unknown: check what they left (logs, files, ports)."
        ),
        (JobTool::Process, ProcessEnd::Exited) => match (end.exit_code, end.signal) {
            (Some(code), _) => format!(
                "{} exited with code {code} after {after}. Read its output with process_read.",
                process()
            ),
            (None, Some(signal)) => format!(
                "{} was killed by signal {signal} after {after}. Read its output with process_read.",
                process()
            ),
            (None, None) => format!(
                "{} ended after {after}. Read its output with process_read.",
                process()
            ),
        },
        (JobTool::Process, ProcessEnd::TimedOut) => format!(
            "{} reached its time limit after {after} and was stopped. Read its output with process_read.",
            process()
        ),
        (JobTool::Process, ProcessEnd::Failed) => format!(
            "{} failed after {after}; process_status says why.",
            process()
        ),
    })
}

/// `command` on one line, cut to [`COMMAND_CHARS`], in backquotes.
fn quoted(command: &str) -> String {
    let line = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut shown: String = line.chars().take(COMMAND_CHARS).collect();
    if line.chars().count() > COMMAND_CHARS {
        shown.push('…');
    }
    format!("`{shown}`")
}

/// `ms` as a person says it: `850ms`, `42s`, `2m13s`, `1h05m`.
fn duration(ms: u64) -> String {
    let seconds = Duration::from_millis(ms).as_secs();
    match seconds {
        0 => format!("{ms}ms"),
        1..60 => format!("{seconds}s"),
        60..3600 => format!("{}m{:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h{:02}m", seconds / 3600, seconds % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn end(tool: JobTool, end: ProcessEnd, exit_code: Option<i32>, signal: Option<i32>) -> JobEnd {
        JobEnd {
            owner: Some(crate::roomkit_owner("room", "agent")),
            tool,
            process_id: (tool == JobTool::Process).then(|| "p3".into()),
            command: "cargo test\n  --all".into(),
            cwd: ".".into(),
            end,
            exit_code,
            signal,
            elapsed_ms: 133_000,
        }
    }

    #[test]
    fn each_end_reads_as_the_python_adapter_writes_it() {
        let cases = [
            (
                end(JobTool::Process, ProcessEnd::Exited, Some(0), None),
                "Background process p3 (`cargo test --all`) exited with code 0 after 2m13s. Read its output with process_read.",
            ),
            (
                end(JobTool::Process, ProcessEnd::Exited, None, Some(9)),
                "Background process p3 (`cargo test --all`) was killed by signal 9 after 2m13s. Read its output with process_read.",
            ),
            (
                end(JobTool::Process, ProcessEnd::TimedOut, None, Some(9)),
                "Background process p3 (`cargo test --all`) reached its time limit after 2m13s and was stopped. Read its output with process_read.",
            ),
            (
                end(JobTool::Process, ProcessEnd::Failed, None, None),
                "Background process p3 (`cargo test --all`) failed after 2m13s; process_status says why.",
            ),
            (
                end(JobTool::Shell, ProcessEnd::Exited, None, None),
                "The background jobs of the shell command `cargo test --all` ended after 2m13s. Their exit status is unknown: check what they left (logs, files, ports).",
            ),
        ];
        for (end, expected) in cases {
            assert_eq!(notice(&end).as_deref(), Some(expected));
        }
        assert_eq!(
            notice(&end(JobTool::Process, ProcessEnd::Stopped, None, Some(9))),
            None
        );
    }

    #[test]
    fn a_long_command_is_cut_and_durations_read_as_spoken() {
        let quoted = quoted(&"x".repeat(250));
        assert_eq!(quoted.chars().count(), COMMAND_CHARS + 3);
        assert!(quoted.ends_with("…`"));
        let spoken: Vec<_> = [850, 42_000, 133_000, 3_900_000]
            .into_iter()
            .map(duration)
            .collect();
        assert_eq!(spoken, ["850ms", "42s", "2m13s", "1h05m"]);
    }

    #[test]
    fn only_a_roomkit_owner_names_a_caller() {
        let owner = crate::roomkit_owner("room", "agent");
        assert_eq!(
            caller(Some(&owner)),
            Some((
                RoomId::new("room").unwrap(),
                ChannelId::new("agent").unwrap()
            ))
        );
        assert_eq!(caller(None), None);
        assert_eq!(caller(Some("standalone")), None);
    }
}
