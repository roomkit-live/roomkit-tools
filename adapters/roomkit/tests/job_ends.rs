//! An agent is told when a background job it started ends, through RoomKit,
//! as an integrator wires it: `notify_job_ends` on the session's ends.
#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

use roomkit::model::{ChannelId, ChannelType, InboundMessage, RoomId, TextContent};
use roomkit::testing::{RecordingTransport, RecordingTransportOptions};
use roomkit::{
    AttachChannelOptions, CreateRoomOptions, ProcessInboundOptions, RoomKit, RoomKitOptions,
};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{AiChannel, AiChannelOptions, Tool, ToolEntry};
use roomkit_tools_process::{JobEnds, ProcessSession, ShellLimits, ShellTool};
use roomkit_tools_roomkit::{ProcessRead, ProcessStart, Shell, notify_job_ends};

/// A kit whose agent answers from `model` with `tools`, steering and
/// serializing its room turns as roomkit-code's agents do, with `ends` told
/// to it; and its room.
#[expect(clippy::unwrap_used, reason = "a broken fixture must fail the test")]
fn kit(model: &ScriptedProvider, tools: Vec<Arc<dyn Tool>>, ends: JobEnds) -> (RoomKit, RoomId) {
    let agent = ChannelId::new("assistant").unwrap();
    let options = AiChannelOptions {
        tools: tools.into_iter().map(ToolEntry::Tool).collect(),
        steer_room_turns: true,
        serialize_room_turns: true,
        ..AiChannelOptions::default()
    };
    let kit = RoomKit::new(RoomKitOptions::default());
    kit.register_channel(AiChannel::new(agent, model.clone(), options).unwrap())
        .unwrap();
    let telling = kit.clone();
    tokio::spawn(async move { notify_job_ends(&telling, ends).await });
    (kit, RoomId::new("room").unwrap())
}

/// The person asks in `room`; returns once the agent's turn ended.
#[expect(clippy::unwrap_used, reason = "a broken fixture must fail the test")]
async fn ask(kit: &RoomKit, room: &RoomId) {
    let chat = RecordingTransport::new(
        ChannelId::new("chat").unwrap(),
        ChannelType::WEBSOCKET,
        RecordingTransportOptions::default(),
    );
    kit.register_channel(chat.clone()).unwrap();
    kit.create_room(CreateRoomOptions {
        room_id: Some(room.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    for channel in [chat.channel_id(), &ChannelId::new("assistant").unwrap()] {
        kit.attach_channel(room, channel, AttachChannelOptions::default())
            .await
            .unwrap();
    }
    let message = InboundMessage::new(
        chat.channel_id().clone(),
        "alice",
        TextContent::new("Start it."),
    );
    let options = ProcessInboundOptions {
        room_id: Some(room.clone()),
        ..Default::default()
    };
    kit.process_inbound(message, options).await.unwrap();
}

/// The model's requests once it was asked `count` times, within five seconds.
#[expect(clippy::panic, reason = "a model never asked must fail the test")]
async fn requests(model: &ScriptedProvider, count: usize) -> Vec<String> {
    tokio::time::timeout(Duration::from_secs(5), async {
        while model.requests().len() < count {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("asked {} times, not {count}", model.requests().len()));
    model
        .requests()
        .iter()
        .map(|request| format!("{request:?}"))
        .collect()
}

#[tokio::test]
async fn a_process_s_exit_gives_its_idle_agent_a_turn() {
    let workspace = tempfile::tempdir().unwrap();
    let session = ProcessSession::new(workspace.path()).await.unwrap();
    let model = ScriptedProvider::new([
        ScriptedRound::calling("", [("c1", "process_start", r#"{"command":"sleep 0.3"}"#)]),
        ScriptedRound::answering("Started."),
        ScriptedRound::answering("It ended."),
    ]);
    let start: Arc<dyn Tool> = Arc::new(ProcessStart::new(session.clone()));
    let (kit, room) = kit(&model, vec![start], session.ends());
    ask(&kit, &room).await;
    let asked = requests(&model, 3).await;
    assert!(!asked[1].contains("Background process"), "{}", asked[1]);
    let told = "Background process p1 (`sleep 0.3`) exited with code 0 after";
    assert!(asked[2].contains(told), "{}", asked[2]);
    assert!(asked[2].contains("Read its output with process_read."));
    assert!(session.close().await);
    kit.close().await.unwrap();
}

#[tokio::test]
async fn a_shell_command_s_background_jobs_are_told_once_ended() {
    let workspace = tempfile::tempdir().unwrap();
    let shell = ShellTool::new(workspace.path())
        .await
        .unwrap()
        .with_limits(ShellLimits {
            keep_background_jobs: true,
            ..ShellLimits::default()
        })
        .unwrap();
    let command = "sleep 0.4 >/dev/null 2>&1 &";
    let arguments = format!(r#"{{"command":"{command}"}}"#);
    let model = ScriptedProvider::new([
        ScriptedRound::calling("", [("c1", "shell", arguments.as_str())]),
        ScriptedRound::answering("Started."),
        ScriptedRound::answering("Seen."),
    ]);
    let tool: Arc<dyn Tool> = Arc::new(Shell::new(shell.clone()));
    let (kit, room) = kit(&model, vec![tool], shell.ends());
    ask(&kit, &room).await;
    let asked = requests(&model, 3).await;
    assert!(
        asked[2].contains(&format!(
            "The background jobs of the shell command `{command}` ended after"
        )),
        "{}",
        asked[2]
    );
    assert!(shell.close().await);
    kit.close().await.unwrap();
}

#[tokio::test]
async fn an_end_during_the_agent_s_turn_is_read_by_that_turn() {
    let workspace = tempfile::tempdir().unwrap();
    let session = ProcessSession::new(workspace.path()).await.unwrap();
    let shell = ShellTool::new(workspace.path()).await.unwrap();
    // The process ends while the turn's next tool, a second's shell, runs.
    let model = ScriptedProvider::new([
        ScriptedRound::calling("", [("c1", "process_start", r#"{"command":"exit 4"}"#)]),
        ScriptedRound::calling("", [("c2", "shell", r#"{"command":"sleep 1"}"#)]),
        ScriptedRound::answering("Done."),
        ScriptedRound::answering("A turn too many."),
    ]);
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(ProcessStart::new(session.clone())),
        Arc::new(Shell::new(shell)),
    ];
    let (kit, room) = kit(&model, tools, session.ends());
    ask(&kit, &room).await;
    roomkit::testing::idle(&kit).await;
    let asked = requests(&model, 3).await;
    assert!(
        asked[2].contains("Background process p1 (`exit 4`) exited with code 4"),
        "{}",
        asked[2]
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        model.requests().len(),
        3,
        "read by the running turn, no turn of its own"
    );
    assert!(session.close().await);
    kit.close().await.unwrap();
}

#[tokio::test]
async fn one_agent_s_turn_does_not_hold_back_another_room_s_notice() {
    let workspace = tempfile::tempdir().unwrap();
    let session = ProcessSession::new(workspace.path()).await.unwrap();
    let shell = ShellTool::new(workspace.path()).await.unwrap();
    // Room a's notice gives a turn that runs a two-second command; room b's
    // notice comes meanwhile.
    let model = ScriptedProvider::new([
        ScriptedRound::calling("", [("c1", "shell", r#"{"command":"sleep 2"}"#)]),
        ScriptedRound::answering("Room b's seen."),
        ScriptedRound::answering("Room a's seen."),
    ]);
    let tool: Arc<dyn Tool> = Arc::new(Shell::new(shell));
    let (kit, _) = kit(&model, vec![tool], session.ends());
    for room in ["room-a", "room-b"] {
        attach(&kit, room).await;
    }
    let cancel = tokio_util::sync::CancellationToken::new();
    for (room, command) in [("room-a", "true"), ("room-b", "sleep 0.3")] {
        let owner = roomkit_tools_roomkit::roomkit_owner(room, "assistant");
        let input = roomkit_tools_process::ProcessStartInput::new(command);
        session.start(Some(&owner), input, &cancel).await.unwrap();
    }
    let asked = tokio::time::timeout(Duration::from_millis(1500), requests(&model, 2))
        .await
        .expect("room b's notice reached the agent while room a's turn ran");
    assert!(
        asked[0].contains("Background process p1 (`true`)"),
        "{}",
        asked[0]
    );
    assert!(
        asked[1].contains("Background process p2 (`sleep 0.3`)"),
        "{}",
        asked[1]
    );
    requests(&model, 3).await;
    assert!(session.close().await);
    kit.close().await.unwrap();
}

/// A room `room` where the person and the agent are.
#[expect(clippy::unwrap_used, reason = "a broken fixture must fail the test")]
async fn attach(kit: &RoomKit, room: &str) {
    let room = RoomId::new(room).unwrap();
    if kit.get_channel(&ChannelId::new("chat").unwrap()).is_none() {
        let chat = RecordingTransport::new(
            ChannelId::new("chat").unwrap(),
            ChannelType::WEBSOCKET,
            RecordingTransportOptions::default(),
        );
        kit.register_channel(chat).unwrap();
    }
    kit.create_room(CreateRoomOptions {
        room_id: Some(room.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    for channel in ["chat", "assistant"] {
        let channel = ChannelId::new(channel).unwrap();
        kit.attach_channel(&room, &channel, AttachChannelOptions::default())
            .await
            .unwrap();
    }
}

/// A model that runs `shell` with `arguments`, answers, then reads the
/// process its notice names and answers again.
fn reading_model(arguments: &str) -> ScriptedProvider {
    ScriptedProvider::new([
        ScriptedRound::calling("", [("c1", "shell", arguments)]),
        ScriptedRound::answering("It runs in the background."),
        ScriptedRound::calling("", [("c2", "process_read", r#"{"process_id":"p1"}"#)]),
        ScriptedRound::answering("Read."),
    ])
}

/// The tools of an agent whose shell runs background commands in `session`.
fn shell_and_reader(shell: ShellTool, session: &ProcessSession) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(Shell::new(shell.with_processes(session.clone()))),
        Arc::new(ProcessRead::new(session.clone())),
    ]
}

#[tokio::test]
async fn a_shell_command_run_in_the_background_is_its_agent_s_process() {
    let workspace = tempfile::tempdir().unwrap();
    let session = ProcessSession::new(workspace.path()).await.unwrap();
    let shell = ShellTool::new(workspace.path()).await.unwrap();
    let command = "sleep 0.3; printf built";
    let model = reading_model(&format!(
        r#"{{"command":"{command}","run_in_background":true}}"#
    ));
    let (kit, room) = kit(&model, shell_and_reader(shell, &session), session.ends());
    ask(&kit, &room).await;
    let asked = requests(&model, 4).await;
    assert!(asked[1].contains("p1 started in the background"), "{}", asked[1]);
    let told = format!("Background process p1 (`{command}`) exited with code 0 after");
    assert!(asked[2].contains(&told), "{}", asked[2]);
    // The agent's own process: its process tools read it.
    assert!(asked[3].contains("built"), "{}", asked[3]);
    assert!(session.close().await);
    kit.close().await.unwrap();
}

#[tokio::test]
async fn a_shell_command_still_running_at_its_timeout_moves_to_its_agent_s_processes() {
    let workspace = tempfile::tempdir().unwrap();
    let session = ProcessSession::new(workspace.path()).await.unwrap();
    let shell = ShellTool::new(workspace.path()).await.unwrap();
    let limits = ShellLimits {
        default_timeout_ms: 200,
        background_on_timeout: true,
        ..ShellLimits::default()
    };
    let command = "printf compiling; sleep 0.5; printf built";
    let model = reading_model(&format!(r#"{{"command":"{command}"}}"#));
    let tools = vec![
        Arc::new(Shell::new(
            shell
                .with_processes(session.clone())
                .with_limits(limits)
                .unwrap(),
        )) as Arc<dyn Tool>,
        Arc::new(ProcessRead::new(session.clone())),
    ];
    let (kit, room) = kit(&model, tools, session.ends());
    ask(&kit, &room).await;
    let asked = requests(&model, 4).await;
    assert!(
        asked[1].contains("moved to the background as p1") && asked[1].contains("compiling"),
        "{}",
        asked[1]
    );
    let told = format!("Background process p1 (`{command}`) exited with code 0 after");
    assert!(asked[2].contains(&told), "{}", asked[2]);
    assert!(asked[3].contains("built"), "{}", asked[3]);
    assert!(session.close().await);
    kit.close().await.unwrap();
}
