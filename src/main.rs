use std::{collections::HashMap, error::Error, future::Pending, net::TcpStream};

use futures_util::Future;
use reqwest::{
    blocking::{Client, Request, RequestBuilder},
    header::HeaderValue,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio_tungstenite::WebSocketStream;
use tungstenite::{stream::MaybeTlsStream, WebSocket};

fn main() {
    eframe::run_native(
        "BaneClient",
        eframe::NativeOptions::default(),
        Box::new(|ctx| Box::new(BaneApp::default())),
    );
}

struct Message {
    message: String,
    from_self: bool,
}

enum State {
    Login(String, String),
    Main {
        selected_id: Option<u64>,
        add_person: String,
        message: String,
    },
}

impl State {
    fn default_main() -> Self {
        Self::Main {
            selected_id: None,
            add_person: String::new(),
            message: String::new(),
        }
    }
}

struct BaneApp {
    state: State,
    chats: HashMap<u64, String>,
    messages: HashMap<u64, Vec<Message>>,
    client: Client,
    ws: Option<(Sender<SendMessage>, Receiver<RecvMessage>)>,
}

impl BaneApp {
    fn send(&mut self) -> Result<(), Box<dyn Error>> {
        let (message, id) = match &self.state {
            State::Main {
                ref message,
                ref selected_id,
                ..
            } => (message.clone(), selected_id.as_ref().unwrap()),
            _ => return Err(String::from("error").into()),
        };
        if let Some(ref mut ws) = self.ws {
            ws.0.blocking_send(SendMessage::Message {
                message: message.clone(),
                reciever: id.clone(),
            });
            self.messages
                .entry(id.clone())
                .or_insert(vec![])
                .push(Message {
                    message,
                    from_self: true,
                });
        } else {
            return Err(String::from("error").into());
        }
        Ok(())
    }

    fn login(&mut self) -> Result<(), Box<dyn Error>> {
        let (name, password) = match &self.state {
            State::Login(ref name, ref password) => (name, password),
            _ => return Err(String::from("error").into()),
        };
        eprintln!("name: {}, password: {}", name, password);
        let res = self
            .client
            .execute(
                self.client
                    .get("https://esfokk.nl/api/v0/login")
                    .header("email", name)
                    .header("password", password)
                    .build()?,
            )?
            .text()
            .unwrap_or(String::new())
            .parse::<Value>()?;
        let mut req = tungstenite::client::IntoClientRequest::into_client_request(
            "wss://esfokk.nl/api/v0/ws",
        )?;
        req.headers_mut().insert(
            "Id",
            res.get("id")
                .unwrap_or(&Value::Null)
                .as_u64()
                .unwrap_or(0)
                .into(),
        );
        req.headers_mut().insert(
            "Token",
            HeaderValue::from_str(
                res.get("token")
                    .unwrap_or(&Value::Null)
                    .as_str()
                    .unwrap_or(""),
            )?,
        );
        eprintln!("req: {:?}", req);
        let ws = tokio_tungstenite::connect_async(req);
        eprintln!("succesfull: {}", res);
        let ch1 = channel(64);
        let ch2 = channel(64);
        std::thread::spawn(move || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let ws1 = ws.await.unwrap();
                websocket_thread(ch2.0, ch1.1, ws1.0).await
            });
        });
        self.ws = Some((ch1.0, ch2.1));
        self.state = State::default_main();
        Ok(())
    }

    fn add_person_by_name(&mut self) -> Result<(), Box<dyn Error>> {
        let name = match &self.state {
            State::Main { add_person, .. } => add_person,
            _ => return Err(String::from("error").into()),
        };
        let res: u64 = self
            .client
            .execute(
                self.client
                    .get("https://esfokk.nl/api/v0/query_name")
                    .header("name", name)
                    .build()?,
            )?
            .text()?
            .parse()?;
        self.chats.insert(res, name.clone());
        Ok(())
    }

    fn add_person_by_id(&mut self, id: Id) -> Result<String, Box<dyn Error>> {
        let value = self
            .client
            .execute(
                self.client
                    .get("https://esfokk.nl/api/v0/query_id")
                    .header("id", id)
                    .build()?,
            )?
            .text()?
            .parse::<Value>()?;
        Ok(format!(
            "{}#{}",
            value.get("name").unwrap().as_str().unwrap(),
            value.get("id").unwrap().as_u64().unwrap(),
        ))
    }
}

impl Default for BaneApp {
    fn default() -> Self {
        Self {
            state: State::Login(String::new(), String::new()),
            chats: HashMap::new(),
            messages: HashMap::new(),
            client: Client::new(),
            ws: None,
        }
    }
}

impl eframe::App for BaneApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut add_id = None;

        if let Some(ref mut ws) = self.ws {
            match ws.1.try_recv() {
                Ok(RecvMessage::Message { sender, message }) => {
                    if !self.chats.contains_key(&sender) {
                        add_id = Some(sender);
                    }
                    eprintln!("{sender}, {message}");
                    self.messages.entry(sender).or_insert(vec![]).push(Message {
                        message,
                        from_self: false,
                    })
                }
                Ok(_) => (),
                Err(_) => (),
            }
        }

        if let Some(id) = add_id {
            self.add_person_by_id(id);
        }

        let mut login_clicked = false;
        let mut add_clicked = false;
        let mut send_clicked = false;
        match self.state {
            State::Main {
                ref mut selected_id,
                ref mut add_person,
                ref mut message,
            } => {
                egui::TopBottomPanel::top("Top Bar").show(ctx, |ui| {
                    ui.menu_button("Add person", |ui| {
                        ui.text_edit_singleline(add_person);
                        add_clicked = ui.button("add").clicked();
                    });
                });
                egui::SidePanel::left("Chats")
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.heading("Chats");
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for (id, name) in self.chats.iter() {
                                if ui.link(name).clicked() {
                                    *selected_id = Some(id.clone())
                                }
                            }
                        });
                    });
                if selected_id.is_some() {
                    egui::TopBottomPanel::bottom("Bottom Panel").show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(message);
                            send_clicked = ui.button("send").clicked();
                        });
                    });
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    if let Some(id) = selected_id {
                        ui.heading(self.chats.get(id).map_or("", |e| e.as_str()));
                        ui.separator();
                        ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                            egui::ScrollArea::vertical()
                                .stick_to_bottom(true)
                                .show(ui, |ui| {
                                    for i in self.messages.get(id).unwrap_or(&vec![]) {
                                        if i.from_self {
                                            ui.label(egui::RichText::new(&i.message).underline());
                                        } else {
                                            ui.label(&i.message);
                                        }
                                    }
                                });
                        });
                    }
                });
            }
            State::Login(ref mut name, ref mut password) => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.text_edit_singleline(name);
                        ui.add(egui::TextEdit::singleline(password).password(true));
                        login_clicked = ui.button("Login").clicked();
                    });
                });
            }
        }
        if login_clicked {
            self.login();
        }
        if add_clicked {
            self.add_person_by_name();
        }
        if send_clicked {
            self.send();
        }
    }
}

type Id = u64;

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum RecvMessage {
    Message { sender: Id, message: String },
    ImageMessage { sender: Id, url: String },
}

#[derive(Serialize, Debug)]
#[serde(tag = "type")]
pub enum SendMessage {
    Message { message: String, reciever: Id },
}

async fn websocket_thread<'a, T>(
    sender: Sender<RecvMessage>,
    reciever: Receiver<SendMessage>,
    ws: WebSocketStream<T>,
) where
    WebSocketStream<T>: futures_util::Sink<tungstenite::Message>
        + futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>>,
    T: Send + 'static,
{
    eprintln!("hello world!");
    use futures_util::{sink::SinkExt, stream::StreamExt, Sink, Stream};
    let (ws_sink, ws_stream) = ws.split();
    let wrapped_reciever = tokio_stream::wrappers::ReceiverStream::new(reciever);
    tokio::spawn(
        ws_stream
            .map(|e| match e.unwrap() {
                tungstenite::Message::Text(t) => serde_json::from_str(&t).unwrap(),
                _ => unimplemented!(),
            })
            .fold(sender, |acc, e| async move {
                eprintln!("{:?}", e);
                acc.send(e).await;
                acc
            }),
    );
    wrapped_reciever
        .map(|e| tungstenite::Message::Text(serde_json::to_string(&e).unwrap()))
        .fold(ws_sink, |mut acc, e| async move {
            eprintln!("{:?}", e);
            acc.send(e).await;
            acc
        })
        .await;
}
