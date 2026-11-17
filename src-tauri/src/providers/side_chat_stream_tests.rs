//! A side chat is a second session with its own agent process (docs/side-chat.md). These run a
//! parent and its side chat as real overlapping child processes speaking the recorded Claude
//! stream protocol: both stream at once, each event carries its own session, the side chat ends or
//! is stopped alone, and the parent's turn goes on to its own end.
use super::*;

/// `$1` picks the part: `parent` streams, then waits on an approval before its last words;
/// `side` streams a short answer and ends; `side-held` streams and then stays open until stopped.
const SCRIPT: &str = r#"
IFS= read -r init
printf '%s\n' '{"type":"control_response","response":{"request_id":"pandacode-init","subtype":"success","response":{}}}'
IFS= read -r prompt
printf '{"type":"system","subtype":"init","session_id":"%s"}\n' "$$"
say() { printf '{"type":"assistant","message":{"id":"%s-%s","content":[{"type":"text","text":"%s"}]}}\n' "$1" "$2" "$3"; }
case "$1" in
  parent)
    say parent 1 'parent one. '
    sleep 0.2
    say parent 2 'parent two. '
    printf '%s\n' '{"type":"control_request","request_id":"parent-approval","request":{"subtype":"can_use_tool","tool_name":"Read","input":{"file_path":"fixture.txt"}}}'
    IFS= read -r response
    say parent 3 'parent three.'
    printf '%s\n' '{"type":"result","subtype":"success","result":"parent three."}'
    ;;
  side)
    say side 1 'side one. '
    sleep 0.1
    say side 2 'side two.'
    printf '%s\n' '{"type":"result","subtype":"success","result":"side two."}'
    ;;
  side-held)
    say held 1 'held one. '
    IFS= read -r never
    ;;
esac
"#;

fn sessions(run: &TestRun) -> HashSet<String> {
    run.observed.lock().unwrap().iter().map(|event| event.session_id.clone()).collect()
}

#[tokio::test]
async fn a_side_chat_streams_beside_its_streaming_parent_and_ends_or_stops_alone() {
    let _serial = process_test_lock().lock().await;
    let mut parent = TestRun::start("parent", SCRIPT);
    let parent_pid = parent.event("session").await.text.parse::<u32>().unwrap();
    assert_eq!(parent.event("text").await.text, "parent one. ");

    // The side chat starts while the parent is mid-stream, as a separate process.
    let mut side = TestRun::start("side", SCRIPT);
    let side_pid = side.event("session").await.text.parse::<u32>().unwrap();
    assert_ne!(side_pid, parent_pid);
    let first = side.event("text").await;
    assert_eq!((first.session_id.as_str(), first.text.as_str()), ("side", "side one. "));
    // Both keep streaming: the parent's next words arrive while the side chat is still open.
    let second = parent.event("text").await;
    assert_eq!((second.session_id.as_str(), second.text.as_str()), ("parent", "parent two. "));
    assert_eq!(side.event("text").await.text, "side two.");
    let approval = parent.event("approval").await.request_id.unwrap();

    // The side chat's turn ends; the parent's does not.
    side.finish().await.unwrap();
    assert!(!parent.task.is_finished());
    assert_eq!(side.text(), "side one. side two.");
    assert_eq!(sessions(&side), HashSet::from(["side".to_string()]));

    // A second question is stopped mid-answer. Stop reaches only that run.
    let mut held = TestRun::start("side-held", SCRIPT);
    let held_pid = held.event("session").await.text.parse::<u32>().unwrap();
    assert_eq!(held.event("text").await.text, "held one. ");
    // The parent's pending approval cannot be answered through the side chat's run.
    held.input.send(RunInput::Approval { request_id: approval.clone(), approved: true }).unwrap();
    held.input.send(RunInput::Stop).unwrap();
    held.finish().await.unwrap();
    assert_eq!(held.text(), "held one. ");
    assert_eq!(sessions(&held), HashSet::from(["side-held".to_string()]));
    assert!(!parent.task.is_finished(), "stopping the side chat left the parent running");
    assert!(process_registry().lock().unwrap().contains(&parent_pid));

    // The parent finishes its own turn, with its own text and nothing from either side run.
    parent.input.send(RunInput::Approval { request_id: approval, approved: true }).unwrap();
    parent.finish().await.unwrap();
    assert_eq!(parent.text(), "parent one. parent two. parent three.");
    assert_eq!(sessions(&parent), HashSet::from(["parent".to_string()]));
    for pid in [parent_pid, side_pid, held_pid] {
        assert!(!process_registry().lock().unwrap().contains(&pid));
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1, "child process leaked");
    }
}
