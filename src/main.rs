use tokio::{
    io::AsyncBufReadExt, io::AsyncWriteExt, io::BufReader, net::TcpListener, sync::broadcast,
};
#[tokio::main]
async fn main() {
    let listener = TcpListener::bind("localhost:3030").await.unwrap();

    let (tx, _rx) = broadcast::channel(10);

    loop {
        let (mut socket, addr) = listener.accept().await.unwrap();

        let tx = tx.clone();
        let mut rx = tx.subscribe();
        tokio::spawn(async move {
            let (reader, mut writer) = socket.split();

            let mut buffer = BufReader::new(reader);
            let mut input_string = String::new();

            loop {
                tokio::select! {
                    result = buffer.read_line(&mut input_string) => {
                        if result.unwrap() == 0 {
                            break;
                        }
                        tx.send((input_string.clone(), addr)).unwrap();
                        input_string.clear();
                    }
                    result = rx.recv() => {
                        let (message, other_addr) = result.unwrap();
                        if addr != other_addr {
                            writer.write_all(message.as_bytes()).await.unwrap();
                        }
                    }

                }
                input_string.clear();
            }
        });
    }
}
