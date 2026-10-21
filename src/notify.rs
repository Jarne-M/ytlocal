//! Windows toast notifications. Best effort: callers ignore failures, the app
//! always shows an in-app message as well.

pub fn toast(title: &str, body: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use tauri_winrt_notification::Toast;
        Toast::new(Toast::POWERSHELL_APP_ID)
            .title(title)
            .text1(body)
            .show()
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = (title, body);
        Ok(())
    }
}
