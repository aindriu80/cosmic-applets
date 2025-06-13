use super::{NetworkManagerEvent, NetworkManagerState};
use cosmic::{
    iced::{self, Subscription},
    iced_futures::stream,
};
use cosmic_dbus_networkmanager::nm::NetworkManager;
use futures::{SinkExt, StreamExt};
use std::{fmt::Debug, hash::Hash};
use zbus::Connection;

pub fn wireless_enabled_subscription<I: 'static + Hash + Copy + Send + Sync + Debug>(
    id: I,
    conn: Connection,
) -> iced::Subscription<NetworkManagerEvent> {
    let initial = State::Continue(conn);
    Subscription::run_with_id(
        id,
        stream::channel(50, move |mut output| {
            let mut state = initial;

            async move {
                loop {
                    state = start_listening(state, &mut output).await;
                }
            }
        }),
    )
}

#[derive(Debug, Clone)]
pub enum State {
    Continue(Connection),
    Error,
}

async fn start_listening(
    state: State,
    output: &mut futures::channel::mpsc::Sender<NetworkManagerEvent>,
) -> State {
    let conn = match state {
        State::Continue(conn) => conn,
        State::Error => iced::futures::future::pending().await,
    };

    let network_manager = match NetworkManager::new(&conn).await {
        Ok(n) => n,
        Err(why) => {
            tracing::error!(why = why.to_string(), "Failed to connect to NetworkManager");
            return State::Error;
        }
    };

    let mut wireless_enabled_changed = network_manager.receive_wireless_enabled_changed().await;

    while let Some(_change) = wireless_enabled_changed.next().await {
        let new_state = NetworkManagerState::new(&conn).await.unwrap_or_default();
        _ = output
            .send(NetworkManagerEvent::WiFiEnabled(new_state))
            .await;
    }
    State::Continue(conn)
}
