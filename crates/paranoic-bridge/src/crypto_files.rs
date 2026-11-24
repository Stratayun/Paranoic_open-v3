//! Plan 1.4, part 1: single-file encryption in the age format (with a
//! password; owner decision 2026-11-14: age only, AES Crypt removed). The result is written to a private spool in
//! the app workspace and committed only after the format verified everything
//! (decryption: authentication over the whole file), then Core copies it into
//! the chosen folder as a NEW file (never replaces). The source is not changed.
use crate::*;
use features::{finish, run_wait, StageSink};
use paranoic_crypto::CryptoError;
use paranoic_formats::{AgeFormat, AgeLimits, AgeSecret};
use zeroize::Zeroizing;

/// `format` of `encrypt_file` / `decrypt_file`. Only age; any other value
/// (2 was AES Crypt, removed) is `Invalid`.
pub const CRYPTO_FORMAT_AGE: u8 = 1;

pub(crate) fn crypto_error(error: CryptoError) -> BridgeError {
    crate::diagnostics::note("Crypto", &format!("{error:?}"));
    match error {
        CryptoError::Authentication => BridgeError::WrongPassword,
        CryptoError::LimitExceeded => BridgeError::Limit,
        CryptoError::InvalidInput => BridgeError::Invalid,
        CryptoError::AlreadyExists => BridgeError::Conflict,
        CryptoError::Unsupported => BridgeError::Unsupported,
        CryptoError::Busy => BridgeError::Busy,
        CryptoError::RateLimited { retry_after_ms } => BridgeError::RateLimited { retry_after_ms },
        CryptoError::Locked => BridgeError::Locked,
        CryptoError::PublicationUncertain | CryptoError::CleanupFailed => BridgeError::Attention,
        _ => BridgeError::Unavailable,
    }
}

/// age scrypt work factor for new files: 2^16 (64 MiB) on phones, 2^18 on desktop.
/// Reading accepts up to 2^18 from any platform.
fn age_limits(creating: bool) -> AgeLimits {
    let factor = if creating && cfg!(target_os = "android") { 16 } else { 18 };
    AgeLimits { max_scrypt_work_factor: factor, ..AgeLimits::default() }
}

impl NativeRuntime {
    fn transform_file(&self, source: NativeLocation, target_parent: NativeLocation, name: String,
        run: impl FnOnce(&mut dyn std::io::Read, Box<dyn paranoic_crypto::StreamSink>) -> std::result::Result<(), CryptoError>) -> Result<()> {
        let source = source.core()?;
        let parent = target_parent.core()?;
        let target = Location { storage: parent.storage, path: parent.path.join(&name)? };
        let mut input = self.core.open_read(&source)?;
        let before = input.metadata()?;
        if before.kind != paranoic_core::EntryKind::File { return Err(BridgeError::Unsupported); }
        let spool = Self::spool_path()?;
        let staged = self.app.stage(&spool, JobId(Uuid::new_v4()))?;
        let result = run(&mut input, Box::new(StageSink(Some(staged)))).map_err(crypto_error).and_then(|_| {
            if input.metadata()? != before { return Err(BridgeError::Changed); }
            let job = self.core.submit(Operation::CopyFile { source: Location { storage: self.app.id(), path: spool.clone() },
                target, conflict: paranoic_core::ConflictPolicy::Fail })?;
            finish(run_wait(&self.core, job.id)?, 1)
        });
        self.drop_spool(&spool);
        result
    }
}

#[uniffi::export]
impl NativeRuntime {
    /// Encrypt `source` with `password` into `target_parent`/`name` (new file).
    /// `format`: 1 age (scrypt). Blocking: call off the UI thread.
    pub fn encrypt_file(&self, source: NativeLocation, target_parent: NativeLocation, name: String, format: u8,
        password: String) -> Result<()> {
        let password = Zeroizing::new(password);
        match format {
            CRYPTO_FORMAT_AGE => self.transform_file(source, target_parent, name, |input, sink|
                AgeFormat::encrypt_password(input, sink, AgeSecret::from(password.to_string()), age_limits(true), |_| Ok(())).map(|_| ())),
            _ => Err(BridgeError::Invalid),
        }
    }
    /// Decrypt `source` into `target_parent`/`name` (new file); nothing is
    /// written unless the whole file authenticated. Blocking: call off the UI thread.
    pub fn decrypt_file(&self, source: NativeLocation, target_parent: NativeLocation, name: String, format: u8,
        password: String) -> Result<()> {
        let password = Zeroizing::new(password);
        match format {
            CRYPTO_FORMAT_AGE => self.transform_file(source, target_parent, name, |input, sink|
                AgeFormat::decrypt_password(input, sink, AgeSecret::from(password.to_string()), age_limits(false), |_| Ok(())).map(|_| ())),
            _ => Err(BridgeError::Invalid),
        }
    }
}
