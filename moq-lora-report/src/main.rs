//! moq-lora-report DIR...  ->  markdown table on stdout (one row per run directory).
//!
//! Each DIR holds what a run left behind: `summary.json` (receiver), `send_summary.json`
//! (sender's last line), `decoded.jsonl[.gz]` and `objects.jsonl[.gz]`. Decoded-frame
//! stalls and glass-to-glass latency are recomputed here with the end-of-run decoder
//! flush removed (the CLI decoder holds one frame until EOF; that frame shows up ~9 s
//! late when the receiver closes it, which is not a stall).
use anyhow::{Context, Result};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

fn open_lines(dir: &Path, name: &str) -> Result<Vec<Value>> {
    let plain = dir.join(name);
    let gz = dir.join(format!("{name}.gz"));
    let r: Box<dyn Read> = if plain.exists() {
        Box::new(std::fs::File::open(&plain)?)
    } else {
        Box::new(flate2::read::GzDecoder::new(std::fs::File::open(&gz).with_context(|| format!("{}", gz.display()))?))
    };
    Ok(BufReader::new(r).lines().map_while(|l| l.ok()).filter_map(|l| serde_json::from_str(&l).ok()).collect())
}

fn json(p: &Path) -> Result<Value> {
    Ok(serde_json::from_str(&std::fs::read_to_string(p).with_context(|| format!("{}", p.display()))?)?)
}

fn pct(v: &mut [f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * q).round() as usize]
}

fn stalls(t: &[u64], thr_ms: f64) -> (u32, f64) {
    let mut n = 0;
    let mut worst = 0f64;
    for w in t.windows(2) {
        let g = (w[1] - w[0]) as f64 / 1000.0;
        worst = worst.max(g);
        if g > thr_ms {
            n += 1;
        }
    }
    (n, worst)
}

fn main() -> Result<()> {
    let dirs: Vec<String> = std::env::args().skip(1).collect();
    println!("| run | dir | FSK | rind | video kbit/s | radio goodput kbit/s | radio frame loss | obj lost | video frame loss | obj lat p50/p95 ms | g2g p50/p95 ms | stalls >250 ms | Pi CPU % | bytes/obj: AEAD / total air overhead | seal / open µs |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for d in dirs {
        let dir = Path::new(&d);
        let r = json(&dir.join("summary.json"))?;
        let s = json(&dir.join("send_summary.json"))?;
        let objs = open_lines(dir, "objects.jsonl")?;
        let dec = open_lines(dir, "decoded.jsonl")?;
        let last_obj = objs.iter().filter_map(|o| o["t_us"].as_u64()).max().unwrap_or(u64::MAX);
        let cutoff = last_obj.saturating_add(1_000_000);
        let mut dt: Vec<u64> = dec.iter().filter_map(|x| x["t_us"].as_u64()).filter(|&t| t <= cutoff).collect();
        dt.sort();
        let mut g2g: Vec<f64> = dec.iter().filter(|x| x["t_us"].as_u64().unwrap_or(0) <= cutoff).filter_map(|x| x["lat_ms"].as_f64()).collect();
        let (dst, dworst) = stalls(&dt, 250.0);
        let f = |v: &Value, k: &str| v[k].as_f64().unwrap_or(f64::NAN);
        let reverse = s["link"].as_str() == Some("catwan") || r["cpu_pct_recv"].is_number() && f(&s, "cpu_pct_total") < 30.0;
        let (dirn, cpu) = if reverse {
            ("Mac→Pi", format!("{:.0} (rx {:.0} + dec {:.0})", f(&r, "cpu_pct_recv") + f(&r, "cpu_pct_children"), f(&r, "cpu_pct_recv"), f(&r, "cpu_pct_children")))
        } else {
            ("Pi→Mac", format!("{:.0} (tx {:.0} + enc {:.0})", f(&s, "cpu_pct_total"), f(&s, "cpu_pct_sender"), f(&s, "cpu_pct_ffmpeg")))
        };
        let ov = &s["overhead_per_object"];
        let prof = r["profile"].as_str().unwrap_or("?");
        let br = prof.split(':').next().unwrap_or("?").parse::<f64>().unwrap_or(0.0) / 1000.0;
        println!(
            "| {} | {dirn} | {br:.0}k{} | {} | {:.1} | {:.1} | {:.3}% | {} | {:.2}% | {:.0} / {:.0} | {:.0} / {:.0} | {} obj, {} dec (worst {:.0} ms) | {cpu} | {} / {:.0} | {:.1} / {:.1} |",
            dir.file_name().unwrap().to_string_lossy(),
            if prof.ends_with(":x") { " (x)" } else { "" },
            s["cipher"].as_str().unwrap_or("none"),
            f(&r, "delivered_video_bps") / 1000.0,
            f(&r, "radio_goodput_bps") / 1000.0,
            100.0 * f(&r, "radio_frame_loss"),
            r["objects_lost"],
            100.0 * f(&r, "video_frame_loss"),
            f(&r["obj_lat_ms"], "p50"), f(&r["obj_lat_ms"], "p95"),
            pct(&mut g2g.clone(), 0.5), pct(&mut g2g, 0.95),
            r["stalls_obj"], dst, dworst.max(f(&r, "worst_gap_obj_ms")),
            ov["aead"], f(ov, "total_air"),
            f(&s, "seal_us_per_object"), f(&r, "open_us_per_object"),
        );
    }
    Ok(())
}
