use anyhow::Result;

use crate::hook::Hook;

pub(crate) async fn extract_go_mod_metadata(_hook: &mut Hook) -> Result<()> {
    Ok(())
}
