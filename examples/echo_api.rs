//! Static files on `/*` except `/api/*`, plus a JSON echo module on `/api/*`.

use std::sync::Arc;

use atomos::config::Config;
use atomos::io::{Out, InOwned};
use atomos::json_out;
use atomos::module::{AsyncModule, BoxFut, Handler};
use atomos::rules::Ruleset;
use atomos::status::Status;
use atomos::{serve, static_router};

struct Echo;

impl AsyncModule for Echo {
    fn name(&self) -> &'static str {
        "api"
    }
    fn handle<'a>(&'a self, req: &'a InOwned) -> BoxFut<'a> {
        Box::pin(async move {
            let payload = serde_json::json!({
                "path": req.path,
                "method": req.method.as_str(),
                "body_bytes": req.body.len(),
            });
            Ok(Out::json(Status::OK, json_out::to_bytes(&payload)))
        })
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "examples/static".into());
    let bind = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "127.0.0.1:8090".into());
    let cfg = Config::from_json(
        format!(
            r#"{{"bind":"{bind}","static_root":"{root}","memory_cap_bytes":67108864,"so_reuseport":true,"tcp_nodelay":true}}"#,
        )
        .as_bytes(),
    )
    .expect("config");
    let rules = Ruleset::parse(
        br#"{"rules":[
          {"id":"s","module":"static","methods":["GET","HEAD"],"include":["/*"],"exclude":["/api/*"]},
          {"id":"a","module":"api","methods":["GET","POST"],"include":["/api/*"],"exclude":[]}
        ]}"#,
    )
    .expect("rules");
    let (router, ctx, _) = static_router(cfg, rules);
    router.insert("api", Handler::Async(Arc::new(Echo)));
    if let Err(e) = serve::run(router, ctx).await {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
