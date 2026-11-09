//! Users by their id, as the system's user database (NSS) knows them: /etc/passwd, and LDAP,
//! sssd or systemd-homed users too.
#![warn(clippy::undocumented_unsafe_blocks)]

/// This process's user id.
pub(crate) fn own_uid() -> u32 {
    // SAFETY: getuid can't fail
    unsafe { libc::getuid() }
}

/// The name of the user `uid`, through NSS.
pub(crate) fn name_of(uid: u32) -> Option<String> {
    let mut buffer = vec![0u8; 4096];

    loop {
        // SAFETY: all-zero is a valid passwd, and getpwuid_r fills it in
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();

        // SAFETY: the pointers are to live locals, and the buffer's length is its own
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                &mut entry,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut found,
            )
        };

        if status == libc::ERANGE && buffer.len() < 1 << 20 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }

        if status != 0 || found.is_null() || entry.pw_name.is_null() {
            return None;
        }

        // SAFETY: getpwuid_r succeeded, so pw_name is a C string in the buffer
        let name = unsafe { std::ffi::CStr::from_ptr(entry.pw_name) };

        return name.to_str().ok().map(String::from);
    }
}

/// This process's user's name.
pub(crate) fn own_name() -> Option<String> {
    name_of(own_uid())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_user_has_a_name() {
        assert!(own_name().is_some_and(|name| !name.is_empty()));
    }
}
