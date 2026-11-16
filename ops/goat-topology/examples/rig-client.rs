// SPDX-License-Identifier: Apache-2.0
//! Rig-only application: one zenoh client that declares a known number of
//! subscribers and publishers on keys of its own, so `rig/rig.sh` can check the
//! per-router counts the page shows against what was declared.
//!
//! Not part of the image (an example is built only on request):
//!
//! ```text
//! cargo build --release --example rig-client
//! rig-client <endpoint> <name> <subscribers> <publishers>
//! ```
//!
//! Keys are `rig/<name>/sub/<i>` and `rig/<name>/pub/<i>`, all distinct, so
//! every declaration is one key expression and one topic. Each publisher puts
//! once a second; nothing subscribes to the whole keyspace.

use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 5 {
        anyhow::bail!("usage: rig-client <endpoint> <name> <subscribers> <publishers>");
    }
    let (endpoint, name) = (&a[1], &a[2]);
    let (nsub, npub): (usize, usize) = (a[3].parse()?, a[4].parse()?);

    let mut config = zenoh::Config::default();
    config
        .insert_json5("mode", "\"client\"")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    config
        .insert_json5("connect/endpoints", &format!("[\"{endpoint}\"]"))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let s = zenoh::open(config)
        .await
        .map_err(|e| anyhow::anyhow!("open: {e}"))?;

    let mut subs = Vec::new();
    for i in 0..nsub {
        subs.push(
            s.declare_subscriber(format!("rig/{name}/sub/{i}"))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        );
    }
    let mut pubs = Vec::new();
    for i in 0..npub {
        pubs.push(
            s.declare_publisher(format!("rig/{name}/pub/{i}"))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        );
    }
    eprintln!("{name}: {nsub} subscribers, {npub} publishers via {endpoint}");
    loop {
        for p in &pubs {
            let _ = p.put("x").await;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
