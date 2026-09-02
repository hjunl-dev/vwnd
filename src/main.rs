mod app;
mod base;

use windows::core::Result;

use crate::base::test_worker_pool;

fn main() -> Result<()> {
    test_worker_pool();
    // app::run()

    Ok(())
}
