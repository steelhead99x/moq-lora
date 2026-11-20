//! Republish what `moq-subghz-recv` delivers as a real MoQ broadcast, using moq.dev's crates.
//!
//! moq-subghz already sends video the MoQ way over the radio: one group per GOP, one object
//! per frame. This maps that 1:1 onto a [hang] broadcast, so any moq.dev relay (a local
//! `moq-relay`, or the moq.pro CDN) can fan it out and the moq.dev web player can watch it.
//!
//! - moq-subghz group `g` becomes MoQ group `g` (unwrapped past 65535) on the `video` track.
//! - moq-subghz object `o` becomes frame `o` of that group, in order.
//! - A gap inside a group closes the MoQ group early and drops the rest of it, the same
//!   rule `GroupGate` applies before the decoder. The next group starts clean.
//! - The catalog (`catalog.json`) is written once the first keyframe brings an SPS.

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use hang::catalog::{Catalog, VideoConfig, H264, PRIORITY};
use hang::container::{Frame, TIMESCALE};
use moq_net::Timestamp;
use moq_subghz_wire::objstream::Record;

/// Track names the moq.dev player looks for.
pub const VIDEO_TRACK: &str = "video";

/// What happened to one record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Push {
    /// Frame 0 of a new MoQ group.
    Started { sequence: u64 },
    /// A later frame of the open group.
    Appended { sequence: u64, frame: u8 },
    /// Dropped: no SPS seen yet, so there is no catalog and nothing a player could decode.
    NoCatalog,
    /// Dropped: the group lost an earlier object (or never had object 0).
    Cut,
    /// Dropped: an older group than the one already published.
    Stale,
}

/// Counters for the end-of-run line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub groups: u64,
    pub frames: u64,
    pub groups_cut: u64,
    pub skipped: u64,
}

struct Open {
    group: u16,
    next_obj: u8,
    producer: moq_net::group::Producer,
}

/// H.264 profile, constraint flags and level from the first SPS in an Annex-B access unit.
pub fn sps_profile(au: &[u8]) -> Option<H264> {
    let nals = moq_subghz_wire::annexb::nals(au);
    let &(start, _) = nals.iter().find(|&&(_, t)| t == 7)?;
    // Skip the start code (3 or 4 bytes) to the NAL header byte.
    let hdr = start
        + if au[start..].starts_with(&[0, 0, 0, 1]) {
            4
        } else {
            3
        };
    let b = au.get(hdr + 1..hdr + 4)?;
    Some(H264 {
        inline: true,
        profile: b[0],
        constraints: b[1],
        level: b[2],
    })
}

/// The catalog for one avc3 (inline SPS/PPS) video track in the legacy container.
pub fn catalog(codec: H264, size: Option<(u32, u32)>) -> Result<Catalog> {
    let mut config = VideoConfig::new(codec);
    if let Some((w, h)) = size {
        config.coded_width = Some(w);
        config.coded_height = Some(h);
    }
    config.optimize_for_latency = Some(true);
    let mut catalog = Catalog::default();
    catalog
        .video
        .insert(VIDEO_TRACK, config)
        .map_err(|e| anyhow!("catalog: {e}"))?;
    Ok(catalog)
}

/// One moq-subghz stream published as a hang broadcast.
pub struct Publisher {
    broadcast: moq_net::broadcast::Producer,
    catalog: moq_net::track::Producer,
    video: moq_net::track::Producer,
    size: Option<(u32, u32)>,
    have_catalog: bool,
    /// Last moq-subghz group seen and its unwrapped MoQ sequence.
    last: Option<(u16, u64)>,
    open: Option<Open>,
    /// Capture time of the first published frame; frame timestamps count from it.
    base_us: Option<u64>,
    pub stats: Stats,
}

impl Publisher {
    /// Create the broadcast `name` on `origin` and announce it. Connect `origin` to a relay
    /// (or read it locally) to deliver it.
    pub fn new(
        origin: &moq_net::origin::Producer,
        name: &str,
        size: Option<(u32, u32)>,
    ) -> Result<Self> {
        let broadcast = origin
            .create_broadcast(name)
            .map_err(|e| anyhow!("broadcast {name}: {e}"))?;
        let catalog = broadcast
            .create_track(Catalog::DEFAULT_NAME, Catalog::default_track_info())
            .map_err(|e| anyhow!("catalog track: {e}"))?;
        let video = broadcast
            .create_track(VIDEO_TRACK, hang::container::track_info(PRIORITY.video))
            .map_err(|e| anyhow!("video track: {e}"))?;
        broadcast
            .announce(Default::default())
            .map_err(|e| anyhow!("announce: {e}"))?;
        Ok(Self {
            broadcast,
            catalog,
            video,
            size,
            have_catalog: false,
            last: None,
            open: None,
            base_us: None,
            stats: Stats::default(),
        })
    }

    /// Unwrapped sequence for `group`, or `None` when it is older than the last one.
    fn sequence(&mut self, group: u16) -> Option<u64> {
        match self.last {
            None => {
                self.last = Some((group, group as u64));
                Some(group as u64)
            }
            Some((g, s)) if g == group => Some(s),
            Some((g, s)) => {
                let delta = group.wrapping_sub(g) as i16;
                if delta <= 0 {
                    return None;
                }
                let s = s + delta as u64;
                self.last = Some((group, s));
                Some(s)
            }
        }
    }

    fn close_open(&mut self) {
        if let Some(o) = self.open.take() {
            // The group may already be closed by a relay dropping it; nothing to do then.
            let _ = o.producer.finish();
        }
    }

    fn write(&mut self, rec: &Record) -> Result<()> {
        let base = *self.base_us.get_or_insert(rec.cap_us);
        let ts = Timestamp::new(rec.cap_us.saturating_sub(base), TIMESCALE)
            .map_err(|e| anyhow!("timestamp: {e}"))?;
        let frame = Frame {
            timestamp: ts,
            payload: Bytes::copy_from_slice(&rec.payload),
        };
        let open = self.open.as_mut().context("no open group")?;
        frame
            .write_to(&mut open.producer)
            .map_err(|e| anyhow!("frame: {e}"))?;
        open.next_obj = open.next_obj.wrapping_add(1);
        self.stats.frames += 1;
        Ok(())
    }

    /// Publish one delivered object.
    pub fn push(&mut self, rec: &Record) -> Result<Push> {
        if !self.have_catalog {
            match (rec.obj, sps_profile(&rec.payload)) {
                (0, Some(codec)) => {
                    let json = catalog(codec, self.size)?
                        .to_vec()
                        .map_err(|e| anyhow!("catalog: {e}"))?;
                    self.catalog
                        .write_frame(Timestamp::now(), json)
                        .map_err(|e| anyhow!("catalog frame: {e}"))?;
                    self.have_catalog = true;
                }
                _ => {
                    self.stats.skipped += 1;
                    return Ok(Push::NoCatalog);
                }
            }
        }
        let Some(sequence) = self.sequence(rec.group) else {
            self.stats.skipped += 1;
            return Ok(Push::Stale);
        };
        if let Some(o) = &self.open {
            if o.group == rec.group && o.next_obj == rec.obj && rec.obj != 0 {
                let frame = rec.obj;
                self.write(rec)?;
                return Ok(Push::Appended { sequence, frame });
            }
            // Anything else ends the open group: a new group, or a gap in this one.
            let gap = o.group == rec.group;
            self.close_open();
            if gap {
                self.stats.groups_cut += 1;
            }
        }
        if rec.obj != 0 {
            self.stats.skipped += 1;
            return Ok(Push::Cut);
        }
        let producer = self
            .video
            .create_group(moq_net::group::Info { sequence })
            .map_err(|e| anyhow!("group {sequence}: {e}"))?;
        self.open = Some(Open {
            group: rec.group,
            next_obj: 0,
            producer,
        });
        self.stats.groups += 1;
        self.write(rec)?;
        Ok(Push::Started { sequence })
    }

    /// Finish the open group and end the tracks.
    pub fn finish(&mut self) {
        self.close_open();
        let _ = self.video.finish();
        let _ = self.catalog.finish();
        self.broadcast.close();
    }
}

/// Synthetic access units for tests: AUD, then SPS/PPS/IDR for a keyframe or one slice.
#[doc(hidden)]
pub fn test_au(key: bool, tag: u8) -> Vec<u8> {
    let mut au = vec![0, 0, 0, 1, 0x09, 0xf0];
    if key {
        au.extend_from_slice(&[0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x1e, 0xda]);
        au.extend_from_slice(&[0, 0, 0, 1, 0x68, 0xce, 0x3c, 0x80]);
        au.extend_from_slice(&[0, 0, 0, 1, 0x65, 0x88, tag]);
    } else {
        au.extend_from_slice(&[0, 0, 0, 1, 0x41, 0x9a, tag]);
    }
    au
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn rec(group: u16, obj: u8) -> Record {
        Record {
            group,
            obj,
            key: obj == 0,
            frame_no: group as u32 * 10 + obj as u32,
            cap_us: 5_000_000 + group as u64 * 1_000_000 + obj as u64 * 66_666,
            payload: test_au(obj == 0, obj),
        }
    }

    #[test]
    fn sps_gives_the_avc3_codec() {
        let c = sps_profile(&test_au(true, 0)).unwrap();
        assert_eq!(c.to_string(), "avc3.42c01e");
        assert!(sps_profile(&test_au(false, 0)).is_none());
        let cat = catalog(c, Some((320, 240))).unwrap();
        let json = String::from_utf8(cat.to_vec().unwrap()).unwrap();
        assert!(json.contains("avc3.42c01e"), "{json}");
        assert!(json.contains("\"codedWidth\":320"), "{json}");
    }

    #[tokio::test]
    async fn groups_map_one_to_one_and_gaps_cut() {
        let origin = moq_tokio::origin::spawn();
        let mut p = Publisher::new(&origin, "t", None).unwrap();
        // Before any keyframe there is no catalog.
        assert_eq!(p.push(&rec(7, 1)).unwrap(), Push::NoCatalog);
        assert_eq!(p.push(&rec(8, 0)).unwrap(), Push::Started { sequence: 8 });
        assert_eq!(
            p.push(&rec(8, 1)).unwrap(),
            Push::Appended {
                sequence: 8,
                frame: 1
            }
        );
        // Object 2 lost: 3 cuts group 8, and 4 stays cut.
        assert_eq!(p.push(&rec(8, 3)).unwrap(), Push::Cut);
        assert_eq!(p.push(&rec(8, 4)).unwrap(), Push::Cut);
        // A group that lost object 0 is skipped whole.
        assert_eq!(p.push(&rec(9, 1)).unwrap(), Push::Cut);
        assert_eq!(p.push(&rec(10, 0)).unwrap(), Push::Started { sequence: 10 });
        // Older group after a newer one.
        assert_eq!(p.push(&rec(9, 0)).unwrap(), Push::Stale);
        assert_eq!(
            p.stats,
            Stats {
                groups: 2,
                frames: 3,
                groups_cut: 1,
                skipped: 5
            }
        );
    }

    #[tokio::test]
    async fn group_ids_unwrap_past_65535() {
        let origin = moq_tokio::origin::spawn();
        let mut p = Publisher::new(&origin, "t", None).unwrap();
        assert_eq!(
            p.push(&rec(65534, 0)).unwrap(),
            Push::Started { sequence: 65534 }
        );
        assert_eq!(
            p.push(&rec(65535, 0)).unwrap(),
            Push::Started { sequence: 65535 }
        );
        assert_eq!(
            p.push(&rec(0, 0)).unwrap(),
            Push::Started { sequence: 65536 }
        );
        assert_eq!(
            p.push(&rec(2, 0)).unwrap(),
            Push::Started { sequence: 65538 }
        );
    }

    #[tokio::test]
    async fn a_local_subscriber_reads_the_same_frames() {
        let origin = moq_tokio::origin::spawn();
        let consumer = origin.consume();
        let mut p = Publisher::new(&origin, "cam", Some((320, 240))).unwrap();
        for g in [3u16, 4] {
            for o in 0..3 {
                p.push(&rec(g, o)).unwrap();
            }
        }
        p.close_open();
        let bc = tokio::time::timeout(Duration::from_secs(2), consumer.request_broadcast("cam"))
            .await
            .unwrap()
            .unwrap();
        let mut cat = bc
            .track(Catalog::DEFAULT_NAME)
            .unwrap()
            .subscribe(None)
            .await
            .unwrap();
        let mut g = cat.recv_group().await.unwrap().unwrap();
        let f = g.read_frame().await.unwrap().unwrap();
        let c: Catalog = Catalog::from_slice(&f.payload).unwrap();
        assert!(c.video.renditions.contains_key(VIDEO_TRACK));

        let mut video = bc
            .track(VIDEO_TRACK)
            .unwrap()
            .subscribe(
                moq_net::track::Subscription::default().with_max_age(Duration::from_secs(30)),
            )
            .await
            .unwrap();
        let mut seen = Vec::new();
        while seen.len() < 2 {
            let mut g = tokio::time::timeout(Duration::from_secs(2), video.recv_group())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let mut frames = Vec::new();
            while let Some(f) = g.read_frame().await.unwrap() {
                frames.push(Frame::decode(f.payload).unwrap());
            }
            seen.push((g.sequence, frames));
        }
        seen.sort_by_key(|s| s.0);
        for ((seq, frames), g) in seen.iter().zip([3u16, 4]) {
            assert_eq!(*seq, g as u64);
            assert_eq!(frames.len(), 3);
            for (o, f) in frames.iter().enumerate() {
                assert_eq!(f.payload.as_ref(), &rec(g, o as u8).payload[..]);
                let want = rec(g, o as u8).cap_us - rec(3, 0).cap_us;
                assert_eq!(f.timestamp.as_micros(), want as u128);
            }
        }
    }
}
