#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
mod appearance;
#[cfg(windows)]
mod native;
#[cfg(windows)]
mod network;
fn main() {
    #[cfg(windows)]
    native::run();
    #[cfg(not(windows))]
    eprintln!("回來的桌面版目前支援 Windows。可在此平台執行 cargo test 驗證資料核心。");
}
