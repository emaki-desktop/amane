//! What polkitd asks, as the shell shows it; always built, so a UI compiles without the
//! agent itself (the `polkit-agent` feature).

use crate::ServiceError;

/// What polkitd is asking the user to prove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRequest {
    pub(crate) id: u64,

    // like org.freedesktop.systemd1.manage-units
    pub(crate) action_id: String,

    // the action's own sentence, like "Authentication is required to start 'sshd.service'."
    pub(crate) message: String,

    // the action's icon name, often empty
    pub(crate) icon: String,

    // the user whose password proves it
    pub(crate) user: String,

    // what will run with the privilege: pkexec's command line, else the asking process's
    pub(crate) command: Option<String>,

    // what the helper asks for right now, once it does
    pub(crate) prompt: Option<AuthPrompt>,

    // lines the helper showed, like a fingerprint reader's
    pub(crate) info: Vec<String>,

    // why the last try failed
    pub(crate) error: Option<ServiceError>,

    // wrong answers so far
    pub(crate) tries: u8,

    // an answer is with the helper, so the window waits
    pub(crate) busy: bool,
}

/// What the helper asks the user to type, like "Password:".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthPrompt {
    pub(crate) text: String,

    // whether what's typed may show, which a password may not
    pub(crate) echo: bool,
}

impl AuthRequest {
    /// What answer() and cancel() take for this request.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The action asked for, like org.freedesktop.systemd1.manage-units.
    pub fn action_id(&self) -> &str {
        &self.action_id
    }

    /// The action's own sentence, like "Authentication is required to start 'sshd.service'."
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The action's icon name, often empty.
    pub fn icon(&self) -> &str {
        &self.icon
    }

    /// The user whose password proves it.
    pub fn user(&self) -> &str {
        &self.user
    }

    /// What will run with the privilege, as polkitd tells it: pkexec's command line (its
    /// arguments included, so a helper's target is seen), else the command line of the
    /// process asking. Control characters are shown as spaces, and a long one is cut short.
    pub fn command(&self) -> Option<&str> {
        self.command.as_deref()
    }

    /// What the helper asks for right now, none until it does.
    pub fn prompt(&self) -> Option<&AuthPrompt> {
        self.prompt.as_ref()
    }

    /// Lines the helper showed, like a fingerprint reader's.
    pub fn info(&self) -> &[String] {
        &self.info
    }

    /// Why the last try failed: `AuthenticationFailed` for a wrong password, else `Failed` with
    /// the helper's own words. The UI words it.
    pub fn error(&self) -> Option<&ServiceError> {
        self.error.as_ref()
    }

    /// Wrong answers so far.
    pub fn tries(&self) -> u8 {
        self.tries
    }

    /// An answer is with the helper, so the window waits.
    pub fn busy(&self) -> bool {
        self.busy
    }
}

impl AuthPrompt {
    /// The helper's question, like "Password:".
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether what's typed may show, which a password may not.
    pub fn echo(&self) -> bool {
        self.echo
    }
}
