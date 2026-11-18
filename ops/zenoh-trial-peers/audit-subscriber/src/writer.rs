//! One batch allocation, independent bounded persistence paths.
use crate::{records::ReplayEvent, state::BatchClock, Metrics};
use anyhow::{Context, Result};
use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use aws_sdk_s3::{config::Region, primitives::ByteStream, Client};
use chrono::Utc;
use std::{path::PathBuf, time::Duration};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

#[derive(Clone)]
pub struct Target {
    pub endpoint: String,
    pub ca_bundle: Option<PathBuf>,
    pub bucket: String,
    pub region: String,
    pub access: String,
    pub secret: String,
}

pub struct WriterArgs {
    pub audit: Option<Target>,
    pub replay: Option<Target>,
    pub subscriber: String,
    pub batch_events: usize,
    pub batch_interval: Duration,
    pub local_dir: Option<PathBuf>,
}

struct Batch {
    key: String,
    bytes: Vec<u8>,
    count: usize,
}

// Storage I/O tasks are independent of relay shards and may migrate threads.
#[allow(clippy::disallowed_methods)]
pub async fn run(
    args: WriterArgs,
    metrics: Metrics,
    mut rx: mpsc::Receiver<ReplayEvent>,
    mut clock: BatchClock,
) -> Result<()> {
    // Separate clients/credentials. No destination or credential fallback.
    let audit_client = client(args.audit.as_ref()).await?;
    let replay_client = client(args.replay.as_ref()).await?;
    let replay_enabled = replay_client.is_some();
    let (audit_tx, audit_rx) = mpsc::channel(4);
    let (replay_tx, replay_rx) = mpsc::channel(4);
    let mut audit_task = tokio::spawn(sink(
        audit_client,
        args.audit.map(|t| t.bucket),
        args.local_dir,
        metrics.clone(),
        audit_rx,
        false,
    ));
    let mut replay_task = tokio::spawn(sink(
        replay_client,
        args.replay.map(|t| t.bucket),
        None,
        metrics.clone(),
        replay_rx,
        true,
    ));
    let mut batch = Vec::with_capacity(args.batch_events);
    let mut interval = tokio::time::interval(args.batch_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut audit_finished = false;
    let mut replay_finished = false;
    let result: Result<()> = async {
        loop {
            tokio::select! {
                result = &mut audit_task => { audit_finished = true; result.context("audit writer panicked")??; anyhow::bail!("audit writer ended unexpectedly"); }
                result = &mut replay_task => { replay_finished = true; result.context("replay writer panicked")??; anyhow::bail!("replay writer ended unexpectedly"); }
                incoming = rx.recv() => {
                    match incoming {
                        Some(event) => {
                            batch.push(event);
                            if batch.len() >= args.batch_events {
                                dispatch(&mut batch, &mut clock, &args.subscriber, replay_enabled, &audit_tx, &replay_tx, &metrics)?;
                            }
                        }
                        None => {
                            dispatch(&mut batch, &mut clock, &args.subscriber, replay_enabled, &audit_tx, &replay_tx, &metrics)?;
                            break;
                        }
                    }
                }
                _ = interval.tick() => {
                    dispatch(&mut batch, &mut clock, &args.subscriber, replay_enabled, &audit_tx, &replay_tx, &metrics)?;
                }
            }
        }
        Ok(())
    }.await;
    drop(audit_tx);
    drop(replay_tx);
    if let Err(e) = result {
        audit_task.abort();
        replay_task.abort();
        if !audit_finished {
            let _ = audit_task.await;
        }
        if !replay_finished {
            let _ = replay_task.await;
        }
        return Err(e);
    }
    // Keep the allocation lock until both sinks finish, preventing a restarted
    // writer from uploading later keys while this writer is still draining.
    let (audit_result, replay_result) = tokio::join!(audit_task, replay_task);
    audit_result.context("audit writer panicked")??;
    replay_result.context("replay writer panicked")??;
    drop(clock);
    Ok(())
}

fn dispatch(
    events: &mut Vec<ReplayEvent>,
    clock: &mut BatchClock,
    subscriber: &str,
    replay: bool,
    audit_tx: &mpsc::Sender<Batch>,
    replay_tx: &mpsc::Sender<Batch>,
    metrics: &Metrics,
) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    let ms = clock.allocate(Utc::now().timestamp_millis())?;
    let (audit, replay_batch) = assemble(events, subscriber, ms, replay)?;
    let count = audit.count;
    match audit_tx.try_send(audit) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            metrics.drop_total.inc_by(count as u64);
            warn!(events = count, "audit batch queue full; batch dropped");
            events.clear();
            return Ok(());
        }
        Err(mpsc::error::TrySendError::Closed(_)) => anyhow::bail!("audit writer channel closed"),
    }
    if let Some(batch) = replay_batch {
        match replay_tx.try_send(batch) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                metrics.replay_drop_total.inc_by(count as u64);
                warn!(
                    events = count,
                    "replay queue full; audit retained, replay unavailable"
                );
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                anyhow::bail!("replay writer channel closed")
            }
        }
    }
    events.clear();
    Ok(())
}

fn assemble(
    events: &mut [ReplayEvent],
    subscriber: &str,
    ms: i64,
    replay: bool,
) -> Result<(Batch, Option<Batch>)> {
    let time =
        chrono::DateTime::from_timestamp_millis(ms).context("batch timestamp out of range")?;
    let mode = if replay { "replay" } else { "meta" };
    let key = format!(
        "{}/{}/{}-{}-v2-{}.ndjson",
        subscriber,
        time.format("%Y/%m/%d/%H"),
        ms,
        events.len(),
        mode
    );
    let mut audit_bytes = Vec::new();
    let mut replay_bytes = Vec::new();
    for (ordinal, event) in events.iter_mut().enumerate() {
        event.audit.event_id = format!("{key}#{ordinal}");
        serde_json::to_writer(&mut audit_bytes, &event.audit)?;
        audit_bytes.push(b'\n');
        if replay {
            serde_json::to_writer(&mut replay_bytes, event)?;
            replay_bytes.push(b'\n');
        }
    }
    let count = events.len();
    let replay_batch = replay.then(|| Batch {
        key: key.clone(),
        bytes: replay_bytes,
        count,
    });
    Ok((
        Batch {
            key,
            bytes: audit_bytes,
            count,
        },
        replay_batch,
    ))
}

pub(crate) async fn client(target: Option<&Target>) -> Result<Option<Client>> {
    let Some(t) = target else {
        return Ok(None);
    };
    anyhow::ensure!(
        !t.access.is_empty() && !t.secret.is_empty(),
        "configured S3 target is missing credentials"
    );
    let endpoint = url::Url::parse(&t.endpoint).context("invalid S3 endpoint")?;
    let fixture_http = cfg!(test) && endpoint.scheme() == "http" && endpoint.host_str() == Some("127.0.0.1");
    anyhow::ensure!(endpoint.scheme() == "https" || fixture_http, "S3 endpoint must use HTTPS");
    anyhow::ensure!(endpoint.username().is_empty() && endpoint.password().is_none() && endpoint.query().is_none() && endpoint.fragment().is_none() && endpoint.path() == "/", "S3 endpoint must be an authority only");
    let mut builder = aws_config::defaults(BehaviorVersion::latest());
    if !fixture_http {
        use aws_smithy_http_client::tls::{self, TlsContext, TrustStore};
        let pem = std::fs::read(t.ca_bundle.as_ref().context("S3 CA bundle required")?).context("read S3 CA bundle")?;
        anyhow::ensure!(!pem.is_empty(), "S3 CA bundle empty");
        let context = TlsContext::builder().with_trust_store(TrustStore::empty().with_pem_certificate(pem)).build()?;
        let http = aws_smithy_http_client::Builder::new()
            .tls_provider(tls::Provider::Rustls(tls::rustls_provider::CryptoMode::AwsLc))
            .tls_context(context).build_https();
        builder = builder.http_client(http);
    }
    let config = builder
        .region(Region::new(t.region.clone()))
        .credentials_provider(Credentials::new(
            t.access.clone(),
            t.secret.clone(),
            None,
            None,
            "audit-subscriber",
        ))
        .endpoint_url(&t.endpoint)
        .load()
        .await;
    let config = aws_sdk_s3::config::Builder::from(&config)
        .force_path_style(true)
        .build();
    Ok(Some(Client::from_conf(config)))
}

async fn sink(
    client: Option<Client>,
    bucket: Option<String>,
    local_dir: Option<PathBuf>,
    metrics: Metrics,
    mut rx: mpsc::Receiver<Batch>,
    replay: bool,
) -> Result<()> {
    while let Some(batch) = rx.recv().await {
        if let Some(dir) = &local_dir {
            // Only the audit sink receives this directory and metadata bytes.
            let path = dir.join(format!("audit-{}.ndjson", Utc::now().format("%Y-%m-%d")));
            use std::io::Write;
            let append = || -> std::io::Result<()> {
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)?;
                f.write_all(&batch.bytes)
            };
            if let Err(e) = append() {
                metrics.local_write_errors_total.inc();
                warn!(error=%e, "local metadata tail append failed");
            }
        }
        let (Some(client), Some(bucket)) = (&client, &bucket) else {
            info!(events = batch.count, "observe-only metadata batch");
            continue;
        };
        // SDK retries complete before the next queued batch. Never schedule a
        // late retry behind an already committed chronological cursor.
        let result = client
            .put_object()
            .bucket(bucket)
            .key(&batch.key)
            .body(ByteStream::from(batch.bytes))
            .content_type("application/x-ndjson")
            .send()
            .await;
        match result {
            Ok(_) => {
                if replay {
                    metrics.replay_batches_uploaded_total.inc();
                } else {
                    metrics.batches_uploaded_total.inc();
                    metrics
                        .batch_size_events
                        .with_label_values(&["uploaded"])
                        .set(batch.count as f64);
                }
            }
            Err(e) => {
                if replay {
                    metrics.replay_write_errors_total.inc();
                    metrics.replay_drop_total.inc_by(batch.count as u64);
                } else {
                    metrics.write_errors_total.inc();
                    metrics.drop_total.inc_by(batch.count as u64);
                    metrics
                        .batch_size_events
                        .with_label_values(&["error"])
                        .set(batch.count as f64);
                }
                error!(error=%e, events=batch.count, replay, "batch upload failed; records unavailable in this sink");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::{record, Capture};

    #[tokio::test]
    #[ignore = "run by the local TLS MinIO fixture with synthetic credentials"]
    async fn native_tls_minio_round_trip() {
        let endpoint = std::env::var("GOAT_FIXTURE_TLS_ENDPOINT").unwrap();
        assert_eq!(url::Url::parse(&endpoint).unwrap().host_str(), Some("127.0.0.1"));
        let target = Target {
            endpoint,
            ca_bundle: Some(std::env::var("GOAT_FIXTURE_TLS_CA").unwrap().into()),
            bucket: "audit-integrity".into(), region: "us-east-1".into(),
            access: "fixture-admin".into(), secret: "fixture-admin-password".into(),
        };
        let storage = client(Some(&target)).await.unwrap().unwrap();
        if std::env::var_os("GOAT_FIXTURE_TLS_WRONG_HOST").is_some() {
            let error = storage.list_objects_v2().bucket(&target.bucket).send().await.unwrap_err();
            let detail = format!("{error:?}");
            assert!(detail.contains("NotValidForName") || detail.contains("not valid") || detail.contains("certificate"), "wrong-host request did not fail certificate validation: {detail}");
            return;
        }
        let mut events = vec![event()];
        let (batch, _) = assemble(&mut events, "site", Utc::now().timestamp_millis(), false).unwrap();
        storage.put_object().bucket(&target.bucket).key(&batch.key).body(ByteStream::from(batch.bytes.clone())).send().await.unwrap();
        let read = storage.get_object().bucket(&target.bucket).key(&batch.key).send().await.unwrap().body.collect().await.unwrap().into_bytes();
        assert_eq!(read.as_ref(), batch.bytes);
        assert!(!String::from_utf8_lossy(&read).contains("payload_bytes_b64"));
        assert!(!String::from_utf8_lossy(&read).contains("private-sentinel"));
        let bad_trust = Target { ca_bundle: Some(std::env::var("GOAT_FIXTURE_TLS_BAD_CA").unwrap().into()), ..target.clone() };
        assert!(client(Some(&bad_trust)).await.unwrap().unwrap().list_objects_v2().bucket(&target.bucket).send().await.is_err());
        let mut plain_url = url::Url::parse(&target.endpoint).unwrap();
        plain_url.set_scheme("http").unwrap();
        plain_url.set_host(Some("localhost")).unwrap();
        let plaintext = Target { endpoint: plain_url.to_string(), ..target };
        assert!(client(Some(&plaintext)).await.is_err());
    }

    fn event() -> ReplayEvent {
        record(
            "a/topic",
            b"private-sentinel",
            None,
            "now".into(),
            Capture {
                sample_kind: "put",
                receiving_site: "fixture",
                policy: &crate::attribution::Policy::default(),
                subscriber: "site",
                identity_model: "per-vendor",
                cap: 16384,
                replay: true,
            },
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[allow(clippy::disallowed_methods)] // Exercise the production standalone I/O supervisor.
    async fn allocation_failure_cancels_inflight_sink_before_releasing_state() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            assert!(socket.read(&mut bytes).await.unwrap() > 0);
            started_tx.send(()).unwrap();
            // Keep the upload in flight until the writer has exited.
            let _ = release_rx.await;
        });
        let dir = std::env::temp_dir().join(format!(
            "goat-audit-cancel-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let clock = BatchClock::open(&dir, "site", Some(0)).unwrap();
        let (tx, rx) = mpsc::channel(2);
        let writer = tokio::spawn(run(
            WriterArgs {
                audit: Some(Target {
                    endpoint,
                    ca_bundle: None,
                    bucket: "audit".into(),
                    region: "us-east-1".into(),
                    access: "fixture".into(),
                    secret: "fixture-secret".into(),
                }),
                replay: None,
                subscriber: "site".into(),
                batch_events: 1,
                batch_interval: Duration::from_secs(60),
                local_dir: None,
            },
            Metrics::new("site").unwrap(),
            rx,
            clock,
        ));
        tx.send(event()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), started_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(BatchClock::open(&dir, "site", None).is_err());
        // Fail the next durable allocation while a sink is still awaiting HTTP.
        std::fs::create_dir(dir.join("watermark.next")).unwrap();
        tx.send(event()).await.unwrap();
        drop(tx);
        let result = tokio::time::timeout(Duration::from_secs(10), writer)
            .await
            .unwrap()
            .unwrap();
        assert!(result.is_err());
        let restarted = BatchClock::open(&dir, "site", None).unwrap();
        drop(restarted);
        release_tx.send(()).unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn failed_replay_upload_preserves_local_metadata_and_audit_upload() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buf = [0; 8192];
                loop {
                    let n = socket.read(&mut buf).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let len: usize = headers
                            .lines()
                            .find_map(|l| {
                                let (key, value) = l.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().unwrap())
                            })
                            .unwrap();
                        if bytes.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                let replay = bytes.starts_with(b"PUT /replay/");
                let status = if replay { "403 Forbidden" } else { "200 OK" };
                write!(
                    socket,
                    "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
                requests.push(bytes);
            }
            requests
        });
        let dir = std::env::temp_dir().join(format!(
            "goat-audit-sink-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let clock = BatchClock::open(&dir.join("state"), "site", Some(0)).unwrap();
        let target = |bucket: &str| Target {
            endpoint: endpoint.clone(),
            ca_bundle: None,
            bucket: bucket.into(),
            region: "us-east-1".into(),
            access: format!("fixture-{bucket}"),
            secret: "fixture-secret".into(),
        };
        let metrics = Metrics::new("site").unwrap();
        let (tx, rx) = mpsc::channel(2);
        tx.send(event()).await.unwrap();
        drop(tx);
        tokio::time::timeout(
            Duration::from_secs(20),
            run(
                WriterArgs {
                    audit: Some(target("audit")),
                    replay: Some(target("replay")),
                    subscriber: "site".into(),
                    batch_events: 1,
                    batch_interval: Duration::from_secs(60),
                    local_dir: Some(dir.clone()),
                },
                metrics.clone(),
                rx,
                clock,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(metrics.batches_uploaded_total.get(), 1);
        assert_eq!(metrics.replay_write_errors_total.get(), 1);
        assert_eq!(metrics.replay_drop_total.get(), 1);
        assert_eq!(metrics.drop_total.get(), 0);
        let requests = server.join().unwrap();
        let audit = requests
            .iter()
            .find(|r| r.starts_with(b"PUT /audit/"))
            .unwrap();
        assert!(!String::from_utf8_lossy(audit).contains("payload_bytes_b64"));
        let local =
            std::fs::read(dir.join(format!("audit-{}.ndjson", Utc::now().format("%Y-%m-%d"))))
                .unwrap();
        assert!(!String::from_utf8_lossy(&local).contains("payload_bytes_b64"));
        assert!(!String::from_utf8_lossy(&local).contains("private-sentinel"));
    }

    #[tokio::test]
    async fn full_replay_queue_never_blocks_audit_and_full_audit_drops_pair() {
        let dir = std::env::temp_dir().join(format!(
            "goat-audit-queue-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let mut clock = BatchClock::open(&dir, "site", Some(0)).unwrap();
        let metrics = Metrics::new("site").unwrap();
        let (a_tx, mut a_rx) = mpsc::channel(1);
        let (r_tx, mut r_rx) = mpsc::channel(1);
        for _ in 0..2 {
            dispatch(
                &mut vec![event()],
                &mut clock,
                "site",
                true,
                &a_tx,
                &r_tx,
                &metrics,
            )
            .unwrap();
            assert!(a_rx.try_recv().is_ok());
        }
        assert_eq!(metrics.replay_drop_total.get(), 1);
        assert_eq!(metrics.drop_total.get(), 0);
        dispatch(
            &mut vec![event()],
            &mut clock,
            "site",
            true,
            &a_tx,
            &r_tx,
            &metrics,
        )
        .unwrap();
        r_rx.try_recv().unwrap();
        dispatch(
            &mut vec![event()],
            &mut clock,
            "site",
            true,
            &a_tx,
            &r_tx,
            &metrics,
        )
        .unwrap();
        assert_eq!(metrics.drop_total.get(), 1);
        assert!(r_rx.try_recv().is_err());
    }
    #[test]
    fn paired_batches_share_ids_but_only_replay_contains_body() {
        let mut events = vec![record(
            "a/topic",
            b"private-sentinel",
            Some("spoof".into()),
            "now".into(),
            Capture {
                sample_kind: "put",
                receiving_site: "fixture",
                policy: &crate::attribution::Policy::default(),
                subscriber: "site",
                identity_model: "per-vendor",
                cap: 16384,
                replay: true,
            },
        )];
        let (a, r) = assemble(&mut events, "site", 1793000000000, true).unwrap();
        let r = r.unwrap();
        assert_eq!(a.key, r.key);
        let audit: serde_json::Value = serde_json::from_slice(&a.bytes).unwrap();
        let replay: serde_json::Value = serde_json::from_slice(&r.bytes).unwrap();
        assert_eq!(audit["event_id"], replay["event_id"]);
        assert!(audit.get("payload_bytes_b64").is_none());
        assert!(replay.get("payload_bytes_b64").is_some());
        assert_eq!(
            assemble(&mut events, "site", 1793000000000, true)
                .unwrap()
                .0
                .key,
            a.key
        );
    }
}
