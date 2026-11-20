//! moq-lora-publish: read the object records `moq-lora-recv --publish` writes on stdin and
//! publish them as a MoQ broadcast on a moq.dev relay or the moq.pro CDN.
use anyhow::{bail, Context, Result};
use clap::Parser;
use moq_lora_publish::{Publisher, Push};
use moq_lora_wire::objstream::{read_record, Record};
use std::io::BufReader;
use std::time::Duration;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Relay URL. Local moq.dev relay: http://localhost:4443/anon.
    /// moq.pro: the URL and ?jwt= token from your project's dashboard.
    #[arg(long, env = "MOQ_LORA_RELAY")]
    url: url::Url,
    /// Broadcast name (the path viewers subscribe to)
    #[arg(long, default_value = "moq-lora")]
    name: String,
    /// Coded picture size for the catalog, WxH (must match the sender's --size)
    #[arg(long, default_value = "320x240")]
    size: String,
    /// Pin a self-signed relay certificate by its SHA-256 (hex) instead of the system roots
    #[arg(long, env = "MOQ_LORA_TLS_FINGERPRINT")]
    tls_fingerprint: Option<String>,
    /// Read records from this file instead of stdin
    #[arg(long)]
    input: Option<String>,
    /// Keep the broadcast up this long after the input ends, so the relay gets the tail
    #[arg(long, default_value_t = 1.0)]
    linger_s: f64,
}

fn parse_size(s: &str) -> Result<(u32, u32)> {
    let (w, h) = s.split_once('x').context("--size is WxH")?;
    Ok((w.parse()?, h.parse()?))
}

#[tokio::main]
async fn main() -> Result<()> {
    let a = Args::parse();
    let size = parse_size(&a.size)?;
    if !matches!(a.url.scheme(), "http" | "https" | "moqt" | "moql") {
        bail!(
            "--url must be http(s):// or moqt:// (got {})",
            a.url.scheme()
        );
    }

    // Records arrive on a blocking reader thread; the radio side never waits on the network.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Record>(256);
    let input = a.input.clone();
    let reader = std::thread::spawn(move || -> Result<()> {
        let src: Box<dyn std::io::Read + Send> = match input {
            Some(p) => Box::new(std::fs::File::open(&p).with_context(|| format!("open {p}"))?),
            None => Box::new(std::io::stdin()),
        };
        let mut r = BufReader::new(src);
        while let Some(rec) = read_record(&mut r)? {
            if tx.blocking_send(rec).is_err() {
                break;
            }
        }
        Ok(())
    });

    let _ = moq_tokio::crypto::install_default();
    let origin = moq_tokio::origin::spawn();
    let mut publisher = Publisher::new(&origin, &a.name, Some(size))?;

    let mut connect = moq_tokio::connect::Config::default();
    connect.url = Some(a.url.clone());
    if let Some(fp) = &a.tls_fingerprint {
        connect.tls.fingerprint = vec![fp.clone()];
    }
    let client = connect
        .init(moq_tokio::quic::Config::default())
        .context("moq client")?
        .with_publisher(&origin);
    let shown = moq_tokio::RedactedUrl::new(&a.url).to_string();
    eprintln!("moq-lora-publish: {shown} broadcast={}", a.name);
    // Reconnects on its own; groups written while it is down are served once it is back.
    let connection = client.connect(a.url.clone());

    loop {
        tokio::select! {
            rec = rx.recv() => {
                let Some(rec) = rec else { break };
                match publisher.push(&rec)? {
                    Push::Started { sequence } if publisher.stats.groups == 1 => {
                        eprintln!("moq-lora-publish: first group {sequence} live");
                    }
                    _ => {}
                }
            }
            res = connection.closed() => {
                res.context("relay connection closed")?;
                bail!("relay connection closed");
            }
        }
    }
    // Input ended. Keep the broadcast up for a moment so viewers and the relay get the
    // last groups, then end it cleanly.
    tokio::time::sleep(Duration::from_secs_f64(a.linger_s)).await;
    publisher.finish();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let s = publisher.stats;
    eprintln!(
        "moq-lora-publish: groups={} frames={} groups_cut={} skipped={}",
        s.groups, s.frames, s.groups_cut, s.skipped
    );
    match reader.join() {
        Ok(r) => r.context("reading records"),
        Err(_) => bail!("record reader panicked"),
    }
}
