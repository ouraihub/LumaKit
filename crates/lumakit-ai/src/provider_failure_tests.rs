use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
};
use tracing_subscriber::layer::SubscriberExt;

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("poisoned"))?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Output {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

#[test]
fn upstream_fields_never_cross_log_or_error_boundary() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let url = format!(
        "http://{}/PRIVATE_URL_PATH?token=PRIVATE_QUERY",
        listener.local_addr()?
    );
    let worker = thread::spawn(move || -> std::io::Result<()> {
        let (mut socket, _) = listener.accept()?;
        socket.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
        let mut bytes = [0; 4096];
        if socket.read(&mut bytes)? == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "request closed",
            ));
        }
        let body = "PRIVATE_BODY_NOT_THE_API_KEY";
        write!(
            socket,
            "HTTP/1.1 429 Too Many Requests\r\nx-request-id: PRIVATE_HEADER_NOT_THE_API_KEY\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )?;
        Ok(())
    });
    let output = Output::default();
    let subscriber = tracing_subscriber::registry()
        .with(lumakit_observability::SpanFields)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .event_format(lumakit_observability::GrepFormat::utc())
                .with_writer(output.clone()),
        );
    let error = tracing::subscriber::with_default(
        subscriber,
        || -> Result<AiError, Box<dyn std::error::Error>> {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            rt.block_on(async {
                let response = reqwest::Client::new().get(url).send().await?;
                Ok(provider_response(response, &CancellationToken::new()).await)
            })
        },
    )?;
    worker.join().map_err(|_| "fixture panic")??;
    let log = String::from_utf8(output.0.lock().map_err(|_| "poisoned")?.clone())?;
    let public = format!("{error:?} {error}");
    for marker in [
        "PRIVATE_URL_PATH",
        "PRIVATE_QUERY",
        "PRIVATE_HEADER_NOT_THE_API_KEY",
        "PRIVATE_BODY_NOT_THE_API_KEY",
    ] {
        assert!(!log.contains(marker), "untrusted value in log");
        assert!(!public.contains(marker), "untrusted value in public error");
    }
    match error {
        AiError::Http {
            status, request_id, ..
        } => {
            assert_eq!(status, 429);
            assert_eq!(
                request_id,
                lumakit_observability::diagnostic_hash("PRIVATE_HEADER_NOT_THE_API_KEY")
            );
        }
        _ => return Err("HTTP classification was lost".into()),
    }
    assert!(log.contains("ai.http.rejected"));
    assert!(log.contains("status:429"));
    Ok(())
}
