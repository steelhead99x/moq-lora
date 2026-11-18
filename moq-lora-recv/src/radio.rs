//! SX1276 on the Dragino HAT, FSK RX (reverse direction: CatWAN -> Pi). The HAT glue is
//! in `moq-lora-pi`.
use anyhow::Result;
use moq_lora_pi::{open_hat, sx_err, Hat};
use sx1276::fsk::FskConfig;

pub struct Radio {
    r: Hat,
}

impl Radio {
    pub fn open(spi_hz: u32, freq_hz: u32, cfg: &FskConfig, power: i8) -> Result<Self> {
        Ok(Radio {
            r: open_hat(spi_hz, freq_hz, cfg, power)?,
        })
    }

    /// Enter continuous FSK receive.
    pub fn start_rx(&mut self) -> Result<()> {
        self.r.fsk_start_receive().map_err(sx_err)
    }

    /// Busy-poll the FIFO for frames > 63 B (the HAT wires no DIO1); returns a payload
    /// or None. `Err` = CRC / FIFO error (counted, not fatal).
    pub fn poll(
        &mut self,
        st: &mut sx1276::FskLongRx,
        now_us: u32,
        stall_us: u32,
    ) -> Result<Option<(Vec<u8>, i16)>, String> {
        match self.r.fsk_poll_long(st, now_us, stall_us) {
            Ok(Some(p)) => Ok(Some((st.payload().to_vec(), p.rssi_dbm))),
            Ok(None) => Ok(None),
            Err(x) => Err(format!("{x:?}")),
        }
    }

    pub fn idle(&mut self) {
        let _ = self.r.standby();
    }
}
