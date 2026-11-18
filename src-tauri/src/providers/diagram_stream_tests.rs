//! Diagrams travel as plain reply text (docs/diagrams.md): the webview draws ```mermaid blocks and
//! shows the stack-diagram skill's picture. These run a real child process speaking the recorded
//! Claude stream protocol and check that both kinds of reply reach the transcript events byte for
//! byte: fences, backticks, arrows, quotes, Unicode and a Markdown image path, split mid-fence
//! across messages the way a streaming agent splits them.
use super::*;

/// `$1` picks the reply: `mermaid` streams a flowchart and a pie chart, split inside the first
/// fence; `skill` streams what the stack-diagram skill's Step 6 delivers.
const SCRIPT: &str = r#"
IFS= read -r init
printf '%s\n' '{"type":"control_response","response":{"request_id":"pandacode-init","subtype":"success","response":{}}}'
IFS= read -r prompt
printf '{"type":"system","subtype":"init","session_id":"%s"}\n' "$$"
case "$1" in
  mermaid)
    printf '%s\n' '{"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"Here is the flow:\n\n```mermaid\nflowchart LR\n  A[\"Client (browser)\"] -->|HTTPS| B{Auth?}\n"}]}}'
    sleep 0.05
    printf '%s\n' '{"type":"assistant","message":{"id":"m2","content":[{"type":"text","text":"  B -- yes --> C[(Postgres)]\n  B -. no .-> D[échec → 401]\n```\n\n```mermaid\npie title Bundle\n  \"main\" : 80\n  \"lazy\" : 120\n```\n"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","result":"done"}'
    ;;
  skill)
    printf '%s\n' '{"type":"assistant","message":{"id":"s1","content":[{"type":"text","text":"![Checkout architecture](/tmp/pandacode-diagrams/checkout/out.png)\n\nThe full page is `/tmp/pandacode-diagrams/checkout/out.html`; icons: svgl / devicon / Lucide."}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","result":"done"}'
    ;;
esac
"#;

const MERMAID_REPLY: &str = "Here is the flow:\n\n```mermaid\nflowchart LR\n  A[\"Client (browser)\"] -->|HTTPS| B{Auth?}\n  B -- yes --> C[(Postgres)]\n  B -. no .-> D[échec → 401]\n```\n\n```mermaid\npie title Bundle\n  \"main\" : 80\n  \"lazy\" : 120\n```\n";
const SKILL_REPLY: &str = "![Checkout architecture](/tmp/pandacode-diagrams/checkout/out.png)\n\nThe full page is `/tmp/pandacode-diagrams/checkout/out.html`; icons: svgl / devicon / Lucide.";

#[tokio::test]
async fn a_mermaid_reply_reaches_the_transcript_intact_even_split_inside_a_fence() {
    let _serial = process_test_lock().lock().await;
    let mut run = TestRun::start("mermaid", SCRIPT);
    let first = run.event("text").await;
    // The first message ends inside the open fence: a streaming view keeps showing source here.
    assert!(first.text.ends_with("-->|HTTPS| B{Auth?}\n"), "{:?}", first.text);
    run.finish().await.unwrap();
    assert_eq!(run.text(), MERMAID_REPLY);
    assert_eq!(run.text().matches("```mermaid\n").count(), 2);
}

#[tokio::test]
async fn the_diagram_skills_picture_and_page_reach_the_transcript_intact() {
    let _serial = process_test_lock().lock().await;
    let mut run = TestRun::start("skill", SCRIPT);
    run.finish().await.unwrap();
    assert_eq!(run.text(), SKILL_REPLY);
}
