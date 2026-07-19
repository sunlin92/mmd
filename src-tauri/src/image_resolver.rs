use std::path::{Path, PathBuf};

use crate::{
    path_auth::{
        canonicalize_existing_path, ensure_authorized_directory_inner,
        ensure_authorized_existing_file_inner, is_authorized_image_path,
    },
    state::AppState,
};

fn decode_percent(input: &str) -> Result<String, String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err("Invalid percent-encoded image path".into());
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .map_err(|_| "Invalid image path".to_string())?;
            let value = u8::from_str_radix(hex, 16)
                .map_err(|_| "Invalid percent-encoded image path".to_string())?;
            out.push(value);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "Image path is not valid UTF-8".into())
}

fn strip_url_fragment_or_query(src: &str) -> &str {
    src.split(['?', '#']).next().unwrap_or(src)
}

fn has_rooted_path_syntax(path: &str) -> bool {
    let bytes = path.as_bytes();
    Path::new(path).is_absolute()
        || matches!(bytes.first(), Some(b'/') | Some(b'\\'))
        || matches!(bytes, [drive, b':', ..] if drive.is_ascii_alphabetic())
}

fn reject_unsafe_relative_image_src(src: &str) -> Result<String, String> {
    let trimmed = strip_url_fragment_or_query(src.trim());
    if trimmed.is_empty() {
        return Err("Image path is empty".into());
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("data:")
        || lower.starts_with("file:")
        || lower.starts_with("//")
    {
        return Err("Only relative local image paths are supported".into());
    }
    let decoded = decode_percent(trimmed)?;
    if has_rooted_path_syntax(&decoded) || decoded.starts_with('~') {
        return Err("Absolute image paths are not allowed".into());
    }
    Ok(decoded)
}

pub(crate) fn resolve_relative_image_path_inner(
    state: &AppState,
    current_file_path: &str,
    workspace_root: Option<&str>,
    image_src: &str,
) -> Result<PathBuf, String> {
    let current_file = ensure_authorized_existing_file_inner(state, current_file_path)?;
    if !current_file.is_file() {
        return Err("Current Markdown path is not a file".into());
    }
    let relative = reject_unsafe_relative_image_src(image_src)?;
    let mut bases = vec![current_file
        .parent()
        .unwrap_or_else(|| Path::new("/"))
        .to_path_buf()];
    if let Some(root) = workspace_root.filter(|r| !r.trim().is_empty()) {
        let root = ensure_authorized_directory_inner(state, root)?;
        if !bases.iter().any(|base| base == &root) {
            bases.push(root);
        }
    }

    for base in bases {
        let candidate = base.join(&relative);
        let canonical = match canonicalize_existing_path(candidate) {
            Ok(canonical) => canonical,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("Image file is not accessible: {error}")),
        };
        if !canonical.is_file() {
            continue;
        }
        if is_authorized_image_path(state, &canonical)? {
            return Ok(canonical);
        }
        return Err("Resolved image escaped authorized roots".into());
    }
    Err("Image file not found".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write};
    use tempfile::tempdir;

    use crate::path_auth::{authorize_directory_root_inner, authorize_file_inner};

    #[test]
    fn rejects_absolute_and_external_image_sources() {
        for rooted in [
            "/tmp/a.png",
            r"\tmp\a.png",
            r"C:\tmp\a.png",
            "C:/tmp/a.png",
            r"C:images\a.png",
            r"\\server\share\a.png",
            "%2Ftmp/a.png",
            "%5Ctmp%5Ca.png",
        ] {
            assert!(
                reject_unsafe_relative_image_src(rooted).is_err(),
                "accepted rooted image source: {rooted}"
            );
        }
        assert!(reject_unsafe_relative_image_src("https://example.com/a.png").is_err());
        assert!(reject_unsafe_relative_image_src("data:image/png;base64,abc").is_err());
        assert!(reject_unsafe_relative_image_src("../a.png").is_ok());
        assert!(reject_unsafe_relative_image_src("%2e%2e/a.png").is_ok());
        assert!(reject_unsafe_relative_image_src("images/a.png").is_ok());
    }

    #[test]
    fn resolves_allowed_relative_image_path() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("doc.md"), "![x](img.png)").unwrap();
        let mut image = fs::File::create(dir.path().join("img.png")).unwrap();
        image.write_all(b"png").unwrap();
        let state = AppState::default();
        let roots = authorize_directory_root_inner(&state, dir.path().to_path_buf()).unwrap();
        authorize_file_inner(&state, dir.path().join("doc.md")).unwrap();
        let resolved = resolve_relative_image_path_inner(
            &state,
            dir.path().join("doc.md").to_str().unwrap(),
            Some(roots.to_str().unwrap()),
            "img.png",
        )
        .unwrap();
        assert_eq!(
            resolved.file_name().and_then(|n| n.to_str()),
            Some("img.png")
        );
    }

    #[test]
    fn resolves_parent_relative_path_inside_the_authorized_workspace() {
        let workspace = tempdir().unwrap();
        let docs = workspace.path().join("docs");
        let workspace_assets = workspace.path().join("assets");
        let sibling_assets = docs.join("assets");
        let doc = docs.join("guide.md");
        let expected = workspace_assets.join("cover.png");
        fs::create_dir_all(&workspace_assets).unwrap();
        fs::create_dir_all(&sibling_assets).unwrap();
        fs::write(&doc, "![cover](../assets/cover.png)").unwrap();
        fs::write(&expected, b"workspace cover").unwrap();
        fs::write(sibling_assets.join("cover.png"), b"sibling cover").unwrap();

        let state = AppState::default();
        let root = authorize_directory_root_inner(&state, workspace.path().to_path_buf()).unwrap();
        authorize_file_inner(&state, doc.clone()).unwrap();
        let resolved = resolve_relative_image_path_inner(
            &state,
            doc.to_str().unwrap(),
            Some(root.to_str().unwrap()),
            "../assets/cover.png",
        )
        .unwrap();

        assert_eq!(resolved, expected.canonicalize().unwrap());
    }

    #[test]
    fn rejects_parent_relative_path_that_escapes_the_authorized_workspace() {
        let outer = tempdir().unwrap();
        let workspace = outer.path().join("workspace");
        let docs = workspace.join("docs");
        let doc = docs.join("guide.md");
        let outside = outer.path().join("cover.png");
        fs::create_dir_all(&docs).unwrap();
        fs::write(&doc, "![cover](../../cover.png)").unwrap();
        fs::write(&outside, b"outside cover").unwrap();

        let state = AppState::default();
        let root = authorize_directory_root_inner(&state, workspace).unwrap();
        authorize_file_inner(&state, doc.clone()).unwrap();
        let error = resolve_relative_image_path_inner(
            &state,
            doc.to_str().unwrap(),
            Some(root.to_str().unwrap()),
            "../../cover.png",
        )
        .unwrap_err();

        assert_eq!(error, "Resolved image escaped authorized roots");
    }

    #[cfg(unix)]
    #[test]
    fn reports_inaccessible_image_instead_of_not_found() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let doc = dir.path().join("doc.md");
        let locked = dir.path().join("locked");
        let image = locked.join("image.png");
        fs::write(&doc, "![x](locked/image.png)").unwrap();
        fs::create_dir(&locked).unwrap();
        fs::write(&image, b"png").unwrap();
        let state = AppState::default();
        authorize_file_inner(&state, doc.clone()).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let result = resolve_relative_image_path_inner(
            &state,
            doc.to_str().unwrap(),
            None,
            "locked/image.png",
        );

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        let error = result.unwrap_err();
        assert!(
            error.starts_with("Image file is not accessible:"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn image_resolver_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let doc = workspace.path().join("doc.md");
        let outside_image = outside.path().join("secret.png");
        let linked_image = workspace.path().join("linked.png");
        fs::write(&doc, "![x](linked.png)").unwrap();
        fs::write(&outside_image, b"not really png").unwrap();
        symlink(&outside_image, &linked_image).unwrap();

        let state = AppState::default();
        authorize_file_inner(&state, doc.clone()).unwrap();

        let result =
            resolve_relative_image_path_inner(&state, doc.to_str().unwrap(), None, "linked.png");
        assert!(result.is_err());
    }
}
