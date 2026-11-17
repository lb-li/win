use std::process::Command;

pub fn open_path(path: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        Command::new("cmd")
            .args(["/C", "start", "", path])
            .spawn()
            .map(|_| ())
    }
    #[cfg(not(windows))]
    {
        Command::new("xdg-open").arg(path).spawn().map(|_| ())
    }
}

pub fn reveal_path(path: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        Command::new("explorer")
            .arg(format!("/select,{path}"))
            .spawn()
            .map(|_| ())
    }
    #[cfg(not(windows))]
    {
        open_path(path)
    }
}

pub fn copy_path(path: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::io::Write;

        let mut child = Command::new("cmd")
            .args(["/C", "clip"])
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(path.as_bytes())?;
        }
        child.wait()?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn non_windows_helpers_do_not_panic() {
        #[cfg(not(windows))]
        {
            let _ = super::copy_path("x");
        }
    }
}
