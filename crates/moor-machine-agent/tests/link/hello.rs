//! The runner judges a hello by the agent's release alone.

use futures_util::SinkExt;
use tungstenite::Message;

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn an_old_agent_naming_a_protocol_is_judged_by_its_release() {
    let w = world().await;
    let mut ws = link::dial(&w.server).await.unwrap();
    let old = json!({"hello": {"machine_id": MACHINE, "protocol": 2, "version": "0.0.1"}});
    ws.send(Message::text(old.to_string())).await.unwrap();
    let answer = link::recv(&mut ws, Duration::from_secs(5)).await.unwrap();
    assert!(matches!(answer, Frame::Challenge { .. }), "{answer:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreadable_hello_gets_update_required_and_nothing_else() {
    let w = world().await;
    let mut ws = link::dial(&w.server).await.unwrap();
    ws.send(Message::text("{\"hi\":1}")).await.unwrap();
    let answer = link::recv(&mut ws, Duration::from_secs(5)).await.unwrap();
    assert!(matches!(answer, Frame::UpdateRequired { .. }), "{answer:?}");
    let after = link::recv(&mut ws, Duration::from_secs(5)).await;
    assert_eq!(after, Err("the link closed".to_owned()));
}
