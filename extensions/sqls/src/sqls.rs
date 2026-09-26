use zed::settings::LspSettings;
use zed_extension_api::{self as zed, LanguageServerId, Result};

const SERVER_NAME: &str = "sqls";

struct SqlsExtension;

impl zed::Extension for SqlsExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let binary_path = LspSettings::for_worktree(SERVER_NAME, worktree)?
            .binary
            .and_then(|binary| binary.path)
            .filter(|path| !path.trim().is_empty());
        let command = binary_path
            .or_else(|| worktree.which(SERVER_NAME))
            .ok_or_else(|| {
                format!(
                    "could not find '{SERVER_NAME}' on PATH; set \
                     `lsp.{SERVER_NAME}.binary.path` to the full executable path or install \
                     the Windows release from \
                     https://github.com/sqls-server/sqls/releases"
                )
            })?;

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::None,
        );

        Ok(zed::Command {
            command,
            args: Vec::new(),
            env: Default::default(),
        })
    }

    fn language_server_workspace_configuration(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        LspSettings::for_worktree(language_server_id.as_ref(), worktree)
            .map(|settings| settings.settings)
    }
}

zed::register_extension!(SqlsExtension);
