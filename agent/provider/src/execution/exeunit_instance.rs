use anyhow::{anyhow, Result};
use derive_more::Display;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use ya_utils_process::ProcessHandle;

/// Working ExeUnit instance representation.
#[derive(Display)]
#[display(fmt = "ExeUnit: name [{}]", name)]
pub struct ExeUnitInstance {
    name: String,
    #[allow(dead_code)]
    working_dir: PathBuf,
    process_handle: ProcessHandle,
}

impl ExeUnitInstance {
    pub fn new(
        name: &str,
        binary_path: &Path,
        working_dir: &Path,
        args: &Vec<String>,
    ) -> Result<ExeUnitInstance> {
        log::info!("Spawning exeunit instance : {}", name);

        let mut command = Command::new(binary_path);
        command.args(args).current_dir(working_dir);

        let child = ProcessHandle::new(&mut command).map_err(|error| {
            anyhow!(
                "Can't spawn ExeUnit [{}] from binary [{}] in working directory [{}]. Error: {}",
                name,
                binary_path.display(),
                working_dir.display(),
                error
            )
        })?;

        log::debug!("Exeunit process spawned, pid: {}", child.pid());

        let instance = ExeUnitInstance {
            name: name.to_string(),
            process_handle: child,
            working_dir: working_dir.to_path_buf(),
        };
        log::info!(
            "Exeunit instance [{}] spawned in workdir {}",
            &instance.name,
            &instance.working_dir.display()
        );

        Ok(instance)
    }

    pub fn kill(&self) {
        log::info!("Killing ExeUnit [{}]... pid: {}", &self.name, self.pid());
        self.process_handle.kill();
    }

    pub async fn terminate(&self, timeout: Duration) -> Result<()> {
        log::info!(
            "Terminating ExeUnit [{}]... pid: {}",
            &self.name,
            self.pid()
        );
        self.process_handle.terminate(timeout).await
    }

    pub fn get_process_handle(&self) -> ProcessHandle {
        self.process_handle.clone()
    }

    fn pid(&self) -> u32 {
        self.process_handle.pid()
    }
}