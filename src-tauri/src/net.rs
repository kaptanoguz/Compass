//! Shared HTTP client and the addresses of outside services. Tests point the
//! addresses at a local server.

use std::time::Duration;

#[derive(Clone)]
pub struct Net {
    pub client: reqwest::Client,
    /// Follows no redirects: Jackett links often redirect to a magnet address.
    pub no_redirect: reqwest::Client,
    pub archive_api: String,
    pub omdb: String,
    pub cinemeta: String,
}

impl Net {
    pub fn new() -> Self {
        Net {
            client: client(),
            no_redirect: no_redirect(),
            archive_api: "https://archive.org/advancedsearch.php".into(),
            omdb: "https://www.omdbapi.com/".into(),
            cinemeta: "https://v3-cinemeta.strem.io".into(),
        }
    }

    /// All outside services served from one test address.
    #[cfg(test)]
    pub fn local(base: &str) -> Self {
        Net {
            client: client(),
            no_redirect: no_redirect(),
            archive_api: format!("{base}/advancedsearch.php"),
            omdb: format!("{base}/omdb/"),
            cinemeta: format!("{base}/cinemeta"),
        }
    }
}

impl Default for Net {
    fn default() -> Self {
        Self::new()
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("Compass/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .expect("http client")
}

fn no_redirect() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("Compass/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .build()
        .expect("http client")
}

/// Error from a search source. `timeout` decides the friendly message.
#[derive(Debug, Clone)]
pub struct SourceError {
    pub timeout: bool,
    pub message: String,
}

impl From<reqwest::Error> for SourceError {
    fn from(e: reqwest::Error) -> Self {
        SourceError { timeout: e.is_timeout(), message: e.to_string() }
    }
}

impl SourceError {
    pub fn msg(m: impl Into<String>) -> Self {
        SourceError { timeout: false, message: m.into() }
    }
}

/// A tiny HTTP server for tests: answers every request with `handler`.
#[cfg(test)]
pub mod testserver {
    use axum::{body::Body, extract::Request, response::Response};
    use std::sync::{Arc, Mutex};

    pub type Handler = Arc<dyn Fn(&Request) -> Response + Send + Sync>;

    pub struct TestServer {
        pub base: String,
        pub seen: Arc<Mutex<Vec<String>>>,
    }

    pub async fn start(handler: impl Fn(&Request) -> Response + Send + Sync + 'static) -> TestServer {
        let handler: Handler = Arc::new(handler);
        let seen = Arc::new(Mutex::new(vec![]));
        let seen2 = seen.clone();
        let app = axum::Router::new().fallback(move |req: Request| {
            let handler = handler.clone();
            let seen = seen2.clone();
            async move {
                seen.lock().unwrap().push(req.uri().to_string());
                handler(&req)
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.ok() });
        TestServer { base, seen }
    }

    pub fn json(v: serde_json::Value) -> Response {
        Response::builder().header("content-type", "application/json").body(Body::from(v.to_string())).unwrap()
    }

    pub fn text(s: &str) -> Response {
        Response::builder().body(Body::from(s.to_string())).unwrap()
    }

    /// Query parameter from a recorded request URI.
    pub fn param(uri: &str, key: &str) -> Option<String> {
        let u = url::Url::parse(&format!("http://x{uri}")).ok()?;
        u.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned())
    }
}
