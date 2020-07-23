use std::collections::HashMap;
use std::sync::Arc;
use warp::filters::ws::{Message, WebSocket, Ws};
use warp::Filter;

use futures_util::future::FutureExt;
use futures_util::stream::StreamExt;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::sync::RwLock;

use serde::{Deserialize, Serialize};

use log::{debug, error, info, log, trace, warn};
use pretty_env_logger;

mod structures;
mod utils;

use structures::*;

#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    info!("Random word {}", utils::getrandomword());

    let wsf = warp::ws();
    let context = Context::default();
    let with_context = warp::any().map(move || context.clone());

    let logg = warp::log("WARP");

    let wshandle = wsf
        .and(with_context)
        .map(|ws: Ws, context| ws.on_upgrade(move |socket| user_connected(socket, context)))
        .with(logg);

    warp::serve(wshandle)
        .run((
            [0, 0, 0, 0],
            std::env::var("PORT")
                .unwrap_or("3012".to_owned())
                .parse()
                .unwrap(),
        ))
        .await;
}

async fn user_connected(websocket: WebSocket, context: Context) {
    info!("Websocket Connection Received");
    println!("Websocket Connection Received");
    let (ws_tx, mut ws_rx) = websocket.split();

    let (tx, rx) = unbounded_channel();
    tokio::task::spawn(rx.forward(ws_tx).map(|result| {
        if let Err(e) = result {
            warn!("websocket send error {:#?}", e);
        }
    }));

    let mut player: Option<Player> = None;

    // TODO: Add timer for closing
    // let timeoutfuture = tokio::time::delay_for(std::time::Duration::from_secs(3));

    if let Some(result) = ws_rx.next().await {
        match result {
            Ok(msg) => {
                if msg.is_binary() {
                    match bincode::deserialize(msg.as_bytes()) {
                        Ok(message) => match message {
                            PlayerMessage::Initialize(id, name) => {
                                info!("Intialize player id {:#} name {:#?}", id, name);
                                player = Some(Player {
                                    id,
                                    name,
                                    send_channel: tx.clone(),
                                    status: PlayerStatus::Initiated,
                                });
                            }
                            _ => {
                                warn!(
                                    "First message not initialize, closing connection {:#?}",
                                    msg
                                );
                                if let Err(e) = tx.send(Ok(Message::close_with(
                                    CloseCodes::WrongInit as u8,
                                    CloseCodes::WrongInit.to_string(),
                                ))) {
                                    error!("Cant close connection {:#?}", e);
                                }
                            }
                        },
                        Err(err) => {
                            debug!("Received message is incorrect format, {:#}", err);
                            if let Err(e) = tx.send(Ok(Message::close_with(
                                CloseCodes::WrongInit as u8,
                                CloseCodes::WrongInit.to_string(),
                            ))) {
                                error!("Cant close connection {:#?}", e);
                            }
                        }
                    }
                } else {
                    error!("First message not binary {:#?}", msg);
                    if let Err(e) = tx.send(Ok(Message::close_with(
                        CloseCodes::WrongInit as u8,
                        CloseCodes::WrongInit.to_string(),
                    ))) {
                        error!("Cant close connection {:#?}", e);
                    }
                }
            }
            Err(e) => {
                warn!("Websocket error {:#?} player : {:#?}", e, player);
            }
        };
    }

    match &mut player {
        Some(player) => {
            if let Some(result) = ws_rx.next().await {
                match result {
                    Ok(msg) => {
                        if msg.is_binary() {
                            match bincode::deserialize(msg.as_bytes()) {
                                Ok(message) => match message {
                                    PlayerMessage::CreateLobby => {
                                        use rand::{distributions::Alphanumeric, Rng};
                                        let lobbyid: String = rand::thread_rng()
                                            .sample_iter(&Alphanumeric)
                                            .take(10)
                                            .collect::<String>();
                                        let privatelobbies =
                                            &mut context.write().await.private_lobbies;
                                        if let Some(_lob) = privatelobbies.get(&lobbyid) {
                                            error!(
                                                "Lobby exist with id {:#?} {:#?}, returning error",
                                                lobbyid, _lob
                                            );
                                            player.close(CloseCodes::CantCreateLobby);
                                        } else {
                                            player.status =
                                                PlayerStatus::JoinedLobby(lobbyid.clone());
                                            let lobby = Lobby::new_with_player(
                                                lobbyid.clone(),
                                                player.clone(),
                                            );
                                            privatelobbies.insert(lobbyid, lobby.clone());

                                            info!("Player {:#?} joined lobby {:#?}", player, lobby);
                                            player.send(SocketMessage::LobbyJoined(lobby));
                                        }
                                    }
                                    PlayerMessage::JoinLobby(lobbyid) => {
                                        let privatelobbies =
                                            &mut context.write().await.private_lobbies;
                                        if let Some(lobby) = privatelobbies.get_mut(&lobbyid) {
                                            player.status =
                                                PlayerStatus::JoinedLobby(lobby.id.clone());
                                            lobby.add_player(player.clone());
                                            info!("Player {:#?} joined lobby {:#?}", player, lobby);
                                            player.send(SocketMessage::LobbyJoined(lobby.clone()));
                                        } else {
                                            player.close(CloseCodes::CantLoinLobbyDoestExist)
                                        }
                                    }
                                    _ => {}
                                },
                                Err(e) => {
                                    log::warn!("Message is not player message {:#?}", e);
                                }
                            }
                        } else {
                            error!("Not binary message {:#?}", msg)
                        }
                    }
                    Err(e) => {
                        log::warn!("Websocket error {:#?}", e);
                    }
                }
            }
        }
        None => {
            warn!("Player not initialized");
            if let Err(e) = tx.send(Ok(Message::close_with(
                CloseCodes::WrongInit as u8,
                CloseCodes::WrongInit.to_string(),
            ))) {
                error!("Cant close connection {:#?}", e);
            }
        }
    }

    if let Some(player) = player {
        if let PlayerStatus::JoinedLobby(lobbyid) = player.status {
            let playerid = player.id.clone();
            let timer = timer_detect(&playerid, &lobbyid, &context);

            let messageblock = websocket_msg(&playerid, &lobbyid, &context, ws_rx);

            tokio::join!(timer, messageblock);
        }
    }
}

async fn player_message(player_id: &str, lobbyid: &str, context: &Context, message: PlayerMessage) {
    let lobbies = &mut context.write().await.private_lobbies;
    if let Some(lobby) = lobbies.get_mut(lobbyid) {
        if let Some(player) = lobby.players.get_mut(player_id) {
            match message {
                PlayerMessage::Chat(chat) => {
                    lobby.chat(player_id, chat);
                }
                PlayerMessage::Ping => {
                    player.send(SocketMessage::Pong);
                }
                PlayerMessage::StartGame => {
                    let pid = &player.id.clone();
                    lobby.start_game(pid);
                }
                PlayerMessage::AddPoints(points) => {
                    lobby.add_points(points);
                }
                PlayerMessage::WordChosen(word) => {
                    if let State::Game(leader,_, data) = &mut lobby.state {
                        if let WordState::ChoseWords(_) = data.word {
                            data.word = WordState::Word(word);
                            lobby.broadcast(SocketMessage::LeaderChange(lobby.state.clone()));
                        }
                    }
                }
                msg => {
                    warn!("Received Unexpected Player message {:#?}", msg);
                }
            }
        } else {
            error!("Player id {:#?} not found in Lobby {:#?}", player_id, lobby);
        }
    } else {
        error!(
            "Lobby id {:#?} not found for player id {:#?}",
            lobbyid, player_id
        );
    }
}

async fn player_disconnect(player_id: &str, lobbyid: &str, context: &Context) {
    log::debug!("Player Disconnected {:#?}", player_id);
    let lobbies = &mut context.write().await.private_lobbies;
    if let Some(lobby) = lobbies.get_mut(lobbyid) {
        lobby.remove_player(player_id);
        if lobby.players.is_empty() {
            let lobid = lobby.id.clone();
            lobbies.remove(&lobid);
        }
    }
}

async fn timer_detect(playerid: &str, lobbyid: &str, context: &Context) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        interval.tick().await;
        if let Some(lobby) = context.write().await.private_lobbies.get_mut(lobbyid) {
            if let Some(player) = lobby.players.get(playerid) {
                if let State::Game(leader,_sc,data)=&mut lobby.state{
                    if let WordState::Word(_)=&data.word{
                        if &player.id == leader{
                            if data.time>=1{
                                data.time-=1;
                                lobby.broadcast(
                                    SocketMessage::TimeUpdate(lobby.state.clone())
                                );
                            }else{
                                lobby.assignnewleader();
                            }
                        }
                    }
                }
            } else {
                warn!("Player doest exist, stopping interval");
                break;
            }
        } else {
            warn!("Lobby doesnt exist, stopping interval");
            break;
        }
    }
}

async fn websocket_msg(
    player_id: &str,
    lobbyid: &str,
    context: &Context,
    mut ws_rx: futures_util::stream::SplitStream<WebSocket>,
) {
    while let Some(msg) = ws_rx.next().await {
        match msg {
            Ok(message) => {
                if message.is_close() {
                    // player_disconnect(&player_id, &lobbyid, &context);
                    break;
                } else if message.is_binary() {
                    match bincode::deserialize(message.as_bytes()) {
                        Ok(player_msg) => {
                            player_message(&player_id, &lobbyid, &context, player_msg).await;
                        }
                        Err(er) => {
                            warn!("Received message not Player Message {:#?}", er);
                        }
                    }
                } else {
                    warn!("Received message not binary {:#?}", message);
                }
            }
            Err(er) => {
                warn!("websocket error {:#}", er);
            }
        }
    }

    player_disconnect(&player_id, &lobbyid, &context).await;
}
