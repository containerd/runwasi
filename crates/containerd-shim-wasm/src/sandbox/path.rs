use std::path::{Path, PathBuf};

/// PathResolve allows to resolve a file path in a set of directories.
pub(crate) trait PathResolve {
    /// Resolve the path of a file give a set of directories as the `which` unix
    /// command would do with components of the `PATH` environment variable, and
    /// return an iterator over all candidates.
    /// Resulting candidates are files that exist, but no other constraint is
    /// imposed, in particular this function does not check for the executable bits.
    /// Further constraints can be added by calling filtering the returned iterator.
    fn resolve_in_dirs(
        &self,
        dirs: impl IntoIterator<Item = impl AsRef<Path>>,
    ) -> impl Iterator<Item = PathBuf>;
    /// Like `resolve_in_dirs`, but searches on the entries of `PATH`.
    fn resolve_in_path(&self) -> impl Iterator<Item = PathBuf>;
    /// Like `resolve_in_dirs`, but searches on the entries of `PATH`, and on `cwd`, in that order.
    fn resolve_in_path_or_cwd(&self) -> impl Iterator<Item = PathBuf>;
}

/// Gets the content of the `PATH` environment variable as an iterator over its components
pub(crate) fn paths() -> impl Iterator<Item = PathBuf> {
    std::env::var_os("PATH")
        .as_ref()
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .into_iter()
}

impl<T: AsRef<Path>> PathResolve for T {
    fn resolve_in_dirs(
        &self,
        dirs: impl IntoIterator<Item = impl AsRef<Path>>,
    ) -> impl Iterator<Item = PathBuf> {
        let cwd = std::env::current_dir().ok();

        let has_separator = self.as_ref().components().count() > 1;

        // The seemingly extra complexity here is because we can only have one concrete
        // return type even if we return an `impl Iterator<Item = PathBuf>`
        let (first, second) = if has_separator {
            // file has a separator, we only need to rºesolve relative to `cwd`, we must ignore `PATH`
            (cwd, None)
        } else {
            // file is just a binary name, we must not resolve relative to `cwd`, but relative to `PATH` components
            let dirs = dirs.into_iter().filter_map(move |p| {
                let path = cwd.as_ref()?.join(p.as_ref()).canonicalize().ok()?;
                path.is_dir().then_some(path)
            });
            (None, Some(dirs))
        };

        let file = self.as_ref().to_owned();
        first
            .into_iter()
            .chain(second.into_iter().flatten())
            .filter_map(move |p| {
                // skip any paths that are not files
                let path = p.join(&file).canonicalize().ok()?;
                path.is_file().then_some(path)
            })
    }

    // Like `find_in_dirs`, but searches on the entries of `PATH`.
    fn resolve_in_path(&self) -> impl Iterator<Item = PathBuf> {
        self.resolve_in_dirs(paths())
    }

    // Like `find_in_dirs`, but searches on the entries of `PATH`, and on `cwd`, in that order.
    fn resolve_in_path_or_cwd(&self) -> impl Iterator<Item = PathBuf> {
        self.resolve_in_dirs(paths().chain(std::env::current_dir().ok()))
    }
}
