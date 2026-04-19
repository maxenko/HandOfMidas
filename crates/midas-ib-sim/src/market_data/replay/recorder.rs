//! Real-IB session recorder (proxy mode). Stage 07 fills in.

use std::path::Path;

/// Records a live IB session to `.dbn` + `.tws.pcap` files. Stage 07 implements.
pub struct SessionRecorder {
    _priv: (),
}

impl SessionRecorder {
    pub fn start(_out_dir: &Path) -> std::io::Result<Self> {
        todo!("Stage 07 — SessionRecorder::start")
    }
}
