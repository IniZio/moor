//! A link the runner holds once its agent is admitted.

use std::sync::Arc;
use std::time::Duration;

use moor_client::link::{self, ops, Channel, Offer, Opened, WebSocketStream};
use tokio::io::{AsyncRead, AsyncWrite};

use super::Links;
use crate::forwards::{accepted, Routes};

/// Holds machine `admitted`'s link, carried with `keepalive`, until it drops or a newer link
/// replaces it, reporting the machine online meanwhile.
pub(super) async fn hold_link<S>(
    ws: WebSocketStream<S>,
    (machine_id, version, offer): (i64, String, Offer),
    keepalive: Duration,
    links: Arc<Links>,
    routes: Arc<Routes>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let carried = link::carry(ws, keepalive, move |line| {
        eprintln!("machine {machine_id}: {line}");
    });
    let (generation, replaced, asks) = links.up(machine_id, version, offer, carried.tell);
    eprintln!("machine {machine_id} linked");
    let why = tokio::select! {
        why = link::drive(carried.socket, link::Mode::Server, asks, |ch| {
            let (routes, links) = (Arc::clone(&routes), Arc::clone(&links));
            tokio::spawn(async move { opened(ch, (machine_id, generation), &routes, &links).await });
        }, std::future::pending()) => why,
        _ = replaced => "a newer link replaced it".to_owned(),
    };
    links.down(machine_id, generation);
    eprintln!("machine {machine_id} unlinked: {why}");
}

/// A stream the agent of a link opened: a listen's connection, or its memory reports.
async fn opened(mut ch: Channel, link: (i64, u64), routes: &Routes, links: &Links) {
    let (machine_id, generation) = link;
    loop {
        match ops::read_line::<_, Opened>(&mut ch).await {
            Ok(Opened::Accepted(a)) => return accepted(ch, machine_id, a, routes).await,
            Ok(Opened::Report { report }) => links.reported(machine_id, generation, &report),
            Err(_) => return,
        }
    }
}
