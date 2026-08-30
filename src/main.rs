use serde::Deserialize;
use std::{io::Read, net::TcpStream};

// Server receives this
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ClientFrame {
    Join { timestamp: String, user: User },
    Say { body: Body },
    Ping,
    Leave,
    Error,
}
#[derive(Debug, Deserialize)]
struct User {
    id: String,
    name: String,
    nick: String,
}

#[derive(Debug, Deserialize)]
struct Body {}

fn main() -> std::io::Result<()> {
    let listener = std::net::TcpListener::bind("127.0.0.1:9521")?;

    for stream in listener.incoming() {
        handle_client(stream)?;
    }

    Ok(())
}

fn handle_client(stream: Result<TcpStream, std::io::Error>) -> Result<(), std::io::Error> {
    let mut buffer = String::new();
    stream?.read_to_string(&mut buffer)?;
    let value: ClientFrame = serde_json::from_str(buffer.as_str()).unwrap_or_else(|e| {
        eprintln!("Error reading data from stream: {:?}", e);
        ClientFrame::Error
    });
    match value {
        ClientFrame::Join { timestamp, user } => {}
        ClientFrame::Say { body } => {}
        ClientFrame::Ping => todo!(),
        ClientFrame::Leave => todo!(),
        ClientFrame::Error => todo!(),
    }
    Ok(())
}

fn handle_message(body: Body) {}
