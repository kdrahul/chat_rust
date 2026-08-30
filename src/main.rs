use serde::Deserialize;
use std::{io::Read, net::TcpStream};

// Server receives this
#[derive(Deserialize)]
enum ClientFrame {
    Join(Join),
    Say { body: Body },
    Ping,
    Leave,
}
struct Message {
    nick: String,
    body: Body,
}
#[derive(Deserialize)]
struct User {
    name: String,
    nick: String,
    id: String,
}

#[derive(Deserialize)]
struct Join {
    timestamp: String,
    user: User,
}

#[derive(Deserialize)]
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
    println!("{buffer}");
    Ok(())
}
