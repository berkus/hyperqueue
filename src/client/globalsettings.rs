use cli_table::ColorChoice;
use std::path::{PathBuf, Path};

pub struct GlobalSettings {
    color_policy: ColorChoice,
    server_dir: PathBuf,
}

impl GlobalSettings {

    pub fn new(server_dir: PathBuf, color_policy: ColorChoice) -> Self {
        GlobalSettings {
            color_policy,
            server_dir,
        }
    }

    pub fn color_policy(&self) -> ColorChoice {
        self.color_policy
    }

    pub fn server_directory(&self) -> &Path {
        &self.server_dir
    }
}