use std::collections::HashMap;
use std::sync::Arc;
use warp::filters::ws::Message;

use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::RwLock;

use serde::{Deserialize, Serialize};

use log::{error, warn};

use std::collections::HashSet;

static DEFAULT_DRAW_TIME: u32 = 90;

#[derive(Default)]
pub struct Lobbies {
    pub private_lobbies: HashMap<String, Lobby>,
}

#[derive(Serialize, Debug, Clone)]
pub struct Lobby {
    pub id: String,
    pub players: HashMap<String, Player>,
    pub state: State,
    pub draw_time: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Point {
    id: u32,
    line_width: u32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    draw: bool,
    color: String,
    eraser: bool,
}

#[derive(Debug, Serialize, Clone)]
pub enum State {
    Lobby(String),
    Game(String, Scores, GameData),
}

#[derive(Debug, Serialize, Clone)]
pub struct GameData {
    pub drawing: Vec<Point>,
    pub guessed: HashSet<String>,
    pub time: u32,
    pub word: WordState,
}

impl GameData {
    pub fn get_score(&self) -> u32 {
        (200_f64 + 800_f64 * (self.time as f64 / DEFAULT_DRAW_TIME as f64)) as u32
    }
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct Scores {
    scores: HashMap<String, u32>,
}

#[derive(Debug, Serialize, Clone)]
pub enum WordState {
    ChoseWords(Vec<String>),
    Word(String),
}

impl GameData {
    pub fn default(lead: &str, time: u32) -> Self {
        let mut hs = HashSet::new();
        hs.insert(lead.to_string());

        let mut words = vec![];
        for _i in 0..3 {
            words.push(crate::utils::getrandomword());
        }
        GameData {
            drawing: vec![],
            guessed: hs,
            word: WordState::ChoseWords(words),
            time,
        }
    }
}

impl State {
    pub fn leader(&self) -> &str {
        match &self {
            State::Lobby(id) => id,
            State::Game(id, _, _) => id,
        }
    }
}

impl Lobby {
    pub fn new_with_player(id: String, player: Player) -> Self {
        let mut map = HashMap::new();
        map.insert(player.id.clone(), player.clone());
        Lobby {
            id,
            players: map,
            state: State::Lobby(player.id.clone()),
            draw_time: DEFAULT_DRAW_TIME,
        }
    }

    pub fn add_player(&mut self, player: Player) -> Self {
        self.broadcast(SocketMessage::PlayerJoined(player.clone()));
        if let Some(oldplayer) = self.players.insert(player.id.clone(), player.clone()) {
            log::warn!("Old player {:#?} replaced by {:#?}", oldplayer, player);
            oldplayer.close(CloseCodes::NewSessionOpened);
        }
        self.clone()
    }

    pub fn broadcast(&self, message: SocketMessage) {
        for p in self.players.iter() {
            p.1.send(message.clone());
        }
    }

    pub fn assignnewleader(&mut self) {
        use itertools::Itertools;
        let mut players = self.players.keys().sorted();
        while let Some(pid) = players.next() {
            if pid == self.state.leader() {
                if let Some(pid) = players.next() {
                    self.state = {
                        match &self.state {
                            State::Game(_, score, _data) => State::Game(
                                pid.clone(),
                                score.clone(),
                                GameData::default(pid, self.draw_time),
                            ),
                            State::Lobby(_) => State::Lobby(pid.clone()),
                        }
                    };
                    self.broadcast(SocketMessage::LeaderChange(self.state.clone()));
                    return;
                }
            }
        }
        if let Some(pid) = self.players.keys().sorted().next() {
            self.state = match &self.state {
                State::Game(_, score, data) => State::Game(
                    pid.clone(),
                    score.to_owned(),
                    GameData::default(pid, self.draw_time),
                ),
                State::Lobby(_) => State::Lobby(pid.clone()),
            };
            self.broadcast(SocketMessage::LeaderChange(self.state.clone()));
        }
    }

    pub fn remove_player(&mut self, playerid: &str) {
        if playerid == self.state.leader() {
            self.assignnewleader();
        }
        if let Some(player) = self.players.remove(playerid) {
            log::debug!("Player removed {:#?}", player);
            self.broadcast(SocketMessage::PlayerDisconnected(player));
        }
    }

    pub fn chat(&mut self, id: &str, mut message: String) {
        if let Some(player) = self.players.get(id) {
            if let State::Game(id, score, data) = &mut self.state {
                let gained_score = data.get_score();
                let drawer_score = 100 + gained_score / self.players.len() as u32;
                match &data.word {
                    WordState::Word(word) => {
                        if message.trim().eq_ignore_ascii_case(word.trim()) {
                            message = "Guessed the word!".to_string();
                            data.guessed.insert(player.id.to_string());
                            score
                                .scores
                                .entry(player.id.clone())
                                .and_modify(|f| *f += gained_score)
                                .or_insert(gained_score);
                            score
                                .scores
                                .entry(id.clone())
                                .and_modify(|f| *f += drawer_score)
                                .or_insert(drawer_score);
                            self.broadcast(SocketMessage::ScoreChange(self.state.clone()));
                        }
                    }
                    WordState::ChoseWords(_words) => {}
                }
            }
            self.broadcast(SocketMessage::Chat(player.name.to_string(), message));
            if self.check_turn_change() {
                self.assignnewleader();
            }
        }
    }

    pub fn check_turn_change(&self) -> bool {
        if let State::Game(id, _, data) = &self.state {
            if data.guessed.len() >= self.players.len() {
                return true;
            }
        }
        false
    }

    pub fn start_game(&mut self, playerid: &str) {
        match &self.state {
            State::Lobby(pid) => {
                if playerid == pid {
                    self.state = State::Game(
                        pid.clone(),
                        Scores::default(),
                        GameData::default(pid, self.draw_time),
                    );
                    self.broadcast(SocketMessage::GameStart(self.state.clone()));
                } else {
                    warn!("Only leader {:#} can start game", pid);
                }
            }
            State::Game(_, _, _) => {
                warn!("Cant start game, already in game state");
            }
        }
    }

    pub fn add_points(&mut self, points: Vec<Point>) {
        match &mut self.state {
            State::Game(leader, _, data) => match data.word {
                WordState::ChoseWords(_) => {}
                WordState::Word(_) => {
                    data.drawing.append(&mut points.clone());
                    self.broadcast(SocketMessage::AddPoints(points))
                }
            },
            _ => warn!("State is in Lobby cant add points"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Player {
    pub id: String,
    pub name: String,
    #[serde(skip)]
    pub send_channel: UnboundedSender<Result<Message, warp::Error>>,
    pub status: PlayerStatus,
}

impl Player {
    pub fn send(&self, message: SocketMessage) {
        match bincode::serialize(&message) {
            Ok(bytes) => {
                if let Err(er) = self.send_channel.send(Ok(Message::binary(bytes))) {
                    log::warn!("Cant send message to player {:#?} error {:#?}", self, er);
                }
            }
            Err(err) => {
                log::error!("Cant serialize {:#?} error {:#?}", message, err);
            }
        }
    }
    pub fn close(&self, code: CloseCodes) {
        if let Err(er) = self
            .send_channel
            .send(Ok(Message::close_with(code as u8, code.to_string())))
        {
            error!(
                "Cant send close message to player {:#?} error {:#?}",
                self, er
            );
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlayerStatus {
    Initiated,
    JoinedLobby(String),
}

#[derive(Debug, Serialize, Copy, Clone)]
pub enum CloseCodes {
    WrongInit,
    CantCreateLobby,
    CantLoinLobbyDoestExist,
    NewSessionOpened,
}
impl std::fmt::Display for CloseCodes {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

pub type Context = Arc<RwLock<Lobbies>>;

#[derive(Debug, Serialize, Deserialize)]
pub enum PlayerMessage {
    Initialize(String, String),
    JoinLobby(String),
    WordChosen(String),
    CreateLobby,
    Ping,

    Chat(String),
    StartGame,

    AddPoints(Vec<Point>),
}

#[derive(Debug, Serialize, Clone)]
pub enum SocketMessage {
    LobbyJoined(Lobby),
    PlayerJoined(Player),
    PlayerDisconnected(Player),
    Close(CloseCodes),

    Chat(String, String),
    LeaderChange(State),
    ScoreChange(State),
    TimeUpdate(State),

    GameStart(State),

    AddPoints(Vec<Point>),

    Pong,
}
