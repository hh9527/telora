use super::{Bind, failure};
use anyhow::Result;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    Request, Response,
    body::{Body, Incoming},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use std::{cell::RefCell, convert::Infallible, rc::Rc, time::Duration};
use tokio::io::{AsyncRead, AsyncWrite};

type Handler<'a> = Rc<RefCell<dyn FnMut(&[u8]) -> Result<Vec<u8>> + 'a>>;

fn response(status: u16, body: Vec<u8>) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body)))
        .unwrap()
}

async fn request(
    req: Request<Incoming>,
    limit: usize,
    handler: Handler<'_>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    if req.method() != hyper::Method::POST && req.method() != hyper::Method::GET {
        let mut reply = response(405, failure("expected GET or POST"));
        reply
            .headers_mut()
            .insert("allow", "GET, POST".parse().unwrap());
        return Ok(reply);
    }
    let method = req.method().as_str().to_owned();
    let path = req.uri().path().to_owned();
    let mut query = std::collections::BTreeMap::<String, Vec<String>>::new();
    for (key, value) in form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes()) {
        query
            .entry(key.into_owned())
            .or_default()
            .push(value.into_owned());
    }
    if req.body().size_hint().lower() > limit as u64 {
        return Ok(response(413, failure("request exceeds input size limit")));
    }
    let body = tokio::time::timeout(
        Duration::from_secs(30),
        Limited::new(req.into_body(), limit).collect(),
    )
    .await;
    let bytes = match body {
        Err(_) => return Ok(response(408, failure("request body timed out"))),
        Ok(Err(error)) => {
            let status = if error.is::<http_body_util::LengthLimitError>() {
                413
            } else {
                400
            };
            return Ok(response(
                status,
                failure("invalid or oversized request body"),
            ));
        }
        Ok(Ok(body)) => body.to_bytes(),
    };
    // Synchronous execution on one thread: only one request can mutate the Guest.
    let input: serde_json::Value = if bytes.is_empty() && method == "GET" {
        serde_json::Value::Null
    } else {
        match serde_json::from_slice(&bytes) {
            Ok(input) => input,
            Err(_) => return Ok(response(400, failure("request body must be JSON"))),
        }
    };
    let envelope =
        serde_json::json!({"http":{"method":method,"path":path,"query":query},"input":input});
    let bytes = serde_json::to_vec(&envelope).expect("HTTP envelope is serializable");
    Ok(match handler.borrow_mut()(&bytes) {
        Ok(reply) => {
            let metadata = serde_json::from_slice::<serde_json::Value>(&reply).ok();
            let status = metadata
                .as_ref()
                .and_then(|value| value.get("httpStatus"))
                .and_then(serde_json::Value::as_u64)
                .filter(|&status| status == 404 || status == 405)
                .unwrap_or(200) as u16;
            let mut result = response(status, reply);
            if status == 405
                && let Some(allow) = metadata
                    .as_ref()
                    .and_then(|value| value.get("allow"))
                    .and_then(serde_json::Value::as_str)
                && let Ok(value) = allow.parse()
            {
                result.headers_mut().insert("allow", value);
            }
            result
        }
        Err(error) => response(500, failure(&error.to_string())),
    })
}

async fn connection<S: AsyncRead + AsyncWrite + Unpin + 'static>(
    stream: S,
    limit: usize,
    handler: Handler<'_>,
) {
    let service = service_fn(move |req| request(req, limit, handler.clone()));
    let mut builder = http1::Builder::new();
    builder.keep_alive(false).max_buf_size(32 * 1024);
    // Connection failures belong to this client, not to the service accept loop.
    let _ = tokio::time::timeout(
        Duration::from_secs(60),
        builder.serve_connection(TokioIo::new(stream), service),
    )
    .await;
}

pub(super) fn serve(
    bind: Bind,
    limit: usize,
    mut transform: impl FnMut(&[u8]) -> Result<Vec<u8>>,
) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    // Scoped local futures allow the service to borrow the already initialized Guest.
    runtime.block_on(async {
        let handler: Handler<'_> = Rc::new(RefCell::new(&mut transform));
        use futures::StreamExt;
        macro_rules! accept {
            ($listener:expr) => {{
                let listener = $listener;
                let mut connections = futures::stream::FuturesUnordered::new();
                loop {
                    tokio::select! {
                        Some(()) = connections.next(), if !connections.is_empty() => {},
                        accepted = listener.accept(), if connections.len() < 64 => {
                            let (stream, _) = accepted?;
                            let handler = handler.clone();
                            connections.push(async move {
                                connection(stream, limit, handler).await;
                            });
                        }
                    }
                }
            }};
        }
        match bind {
            Bind::Http(addr) => accept!(tokio::net::TcpListener::bind(addr).await?),
            #[cfg(unix)]
            Bind::Unix(path) => accept!(tokio::net::UnixListener::bind(path)?),
            _ => anyhow::bail!("unsupported HTTP transport"),
        }
    })
}
