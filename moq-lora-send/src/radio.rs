//! SX1276 on the Dragino HAT, FSK TX only. The HAT glue is in `moq-lora-pi`.
use anyhow::{bail, Result};
use moq_lora_pi::{open_hat, sx_err, Hat};
use sx1276::fsk::FskConfig;

pub struct Radio {
    r: Hat,
    toa_us: u32,
}

impl Radio {
    pub fn open(spi_hz: u32, freq_hz: u32, cfg: &FskConfig, power: i8) -> Result<Self> {
        Ok(Radio {
            r: open_hat(spi_hz, freq_hz, cfg, power)?,
            toa_us: cfg.time_on_air_us(255),
        })
    }

    /// Transmit one frame and wait for PacketSent.
    pub fn send(&mut self, f: &[u8]) -> Result<()> {
        if f.len() > 63 {
            self.r.fsk_transmit_long(f, 1_000_000).map_err(sx_err)?;
        } else {
            self.r.fsk_start_transmit(f).map_err(sx_err)?;
        }
        if !self
            .r
            .wait_dio0(self.toa_us * 3 + 200_000)
            .map_err(sx_err)?
        {
            bail!("TX timeout");
        }
        if !self.r.fsk_finish_transmit().map_err(sx_err)? {
            bail!("PacketSent flag missing");
        }
        Ok(())
    }

    pub fn idle(&mut self) {
        let _ = self.r.standby();
    }
}
