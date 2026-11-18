use return_note::ai::{
    Classification, parse_with_labels, request_body, service_error, valid_model,
};
use std::{
    ffi::c_void,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{Networking::WinHttp::*, Security::Credentials::*};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn target() -> Vec<u16> {
    wide(if std::env::var_os("RETURN_NOTE_DATA").is_some() {
        "ReturnNote.Test.Gemini"
    } else {
        "ReturnNote.Gemini"
    })
}
pub fn read_key() -> Result<Option<String>, String> {
    unsafe {
        let mut p = null_mut();
        if CredReadW(target().as_ptr(), CRED_TYPE_GENERIC, 0, &mut p) == 0 {
            return if windows_sys::Win32::Foundation::GetLastError()
                == windows_sys::Win32::Foundation::ERROR_NOT_FOUND
            {
                Ok(None)
            } else {
                Err("無法讀取 Windows 認證管理員。".into())
            };
        }
        let bytes =
            std::slice::from_raw_parts((*p).CredentialBlob, (*p).CredentialBlobSize as usize);
        let result = String::from_utf8(bytes.to_vec()).map_err(|_| "API key 無法讀取。".into());
        CredFree(p.cast());
        result.map(Some)
    }
}
pub fn save_key(key: &str) -> Result<(), String> {
    if key.trim().is_empty() || key.len() > 2000 {
        return Err("請填入有效的 Google API key。".into());
    }
    let mut name = target();
    let mut user = wide("ReturnNote");
    let mut bytes = key.trim().as_bytes().to_vec();
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: name.as_mut_ptr(),
        CredentialBlobSize: bytes.len() as u32,
        CredentialBlob: bytes.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: user.as_mut_ptr(),
        ..unsafe { std::mem::zeroed() }
    };
    let ok = unsafe { CredWriteW(&credential, 0) };
    bytes.fill(0);
    if ok == 0 {
        Err("API key 沒存好，請檢查 Windows 認證管理員。".into())
    } else {
        Ok(())
    }
}
pub fn delete_key() -> Result<(), String> {
    if unsafe { CredDeleteW(target().as_ptr(), CRED_TYPE_GENERIC, 0) } == 0
        && unsafe { windows_sys::Win32::Foundation::GetLastError() }
            != windows_sys::Win32::Foundation::ERROR_NOT_FOUND
    {
        Err("API key 未能移除。".into())
    } else {
        Ok(())
    }
}
struct Handle(*mut c_void);
impl Handle {
    fn new(h: *mut c_void) -> Result<Self, String> {
        if h.is_null() {
            Err("無法建立 AI 連線，原文已存好。".into())
        } else {
            Ok(Self(h))
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            WinHttpCloseHandle(self.0);
        }
    }
}

pub fn classify(
    text: &str,
    model: &str,
    key: &str,
    labels: &[String],
) -> Result<Classification, String> {
    if !valid_model(model) {
        return Err("模型名稱無效。".into());
    }
    let body = serde_json::to_vec(&request_body(text, labels)).map_err(|_| "無法建立分類請求。")?;
    unsafe {
        let session = Handle::new(WinHttpOpen(
            wide("ReturnNote/0.3").as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            null(),
            null(),
            0,
        ))?;
        WinHttpSetTimeouts(session.0, 5000, 5000, 10000, 15000);
        let conn = Handle::new(WinHttpConnect(
            session.0,
            wide("generativelanguage.googleapis.com").as_ptr(),
            443,
            0,
        ))?;
        let path = wide(&format!("/v1beta/models/{model}:generateContent"));
        let req = Handle::new(WinHttpOpenRequest(
            conn.0,
            wide("POST").as_ptr(),
            path.as_ptr(),
            null(),
            null(),
            null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        // Never forward credentials through an HTTP redirect.
        let redirect = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
        if WinHttpSetOption(
            req.0,
            WINHTTP_OPTION_REDIRECT_POLICY,
            (&redirect as *const u32).cast(),
            4,
        ) == 0
        {
            return Err("無法設定安全連線。".into());
        }
        let headers = wide(&format!(
            "Content-Type: application/json\r\nx-goog-api-key: {key}\r\n"
        ));
        if key.contains(['\r', '\n']) {
            return Err("API key 格式不正確。".into());
        }
        if WinHttpSendRequest(
            req.0,
            headers.as_ptr(),
            u32::MAX,
            body.as_ptr().cast(),
            body.len() as u32,
            body.len() as u32,
            0,
        ) == 0
            || WinHttpReceiveResponse(req.0, null_mut()) == 0
        {
            return Err("AI 連線逾時或失敗，原文已存好；不會自動重送。".into());
        }
        let mut status = 0u32;
        let mut len = 4u32;
        if WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&mut status as *mut u32).cast(),
            &mut len,
            null_mut(),
        ) == 0
        {
            return Err("無法確認 AI 回覆。".into());
        }
        let mut response = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let mut read = 0;
            if WinHttpReadData(
                req.0,
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                &mut read,
            ) == 0
            {
                return Err("AI 回覆中斷，原文已保留。".into());
            }
            if read == 0 {
                break;
            }
            response.extend_from_slice(&chunk[..read as usize]);
            if response.len() > 131072 {
                return Err("AI 回覆過大，原文已保留。".into());
            }
        }
        if status != 200 {
            let error = serde_json::from_slice(&response).unwrap_or(serde_json::Value::Null);
            return Err(service_error(status, &error));
        }
        let value = serde_json::from_slice(&response).map_err(|_| "AI 回覆格式無法讀取。")?;
        parse_with_labels(&value, labels)
    }
}
