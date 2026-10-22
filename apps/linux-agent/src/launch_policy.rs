use anyhow::{bail, Result};

/// Never choose an arbitrary window or start another instance when selection is
/// ambiguous. The controller can select a specific row from window.list.
pub fn unique<T>(mut candidates: impl Iterator<Item = T>) -> Result<Option<T>> {
    let first = candidates.next();
    if candidates.next().is_some() {
        bail!("application has multiple windows")
    }
    Ok(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absence_allows_launch_but_ambiguity_never_does() {
        assert_eq!(unique(std::iter::empty::<u8>()).unwrap(), None);
        assert_eq!(unique([7].into_iter()).unwrap(), Some(7));
        assert!(unique([7, 8].into_iter()).is_err());
    }
}
