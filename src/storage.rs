use crate::model::Board;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub struct Storage {
    pub path: PathBuf,
}
impl Storage {
    pub fn new(directory: &Path) -> Result<Self, String> {
        fs::create_dir_all(directory).map_err(|_| "無法建立便條資料夾。")?;
        Ok(Self {
            path: directory.join("notes.json"),
        })
    }
    pub fn load(&self) -> Result<Board, String> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                let mut board: Board = serde_json::from_slice(&bytes).map_err(
                    |_| "便條資料無法讀取，原檔未被改寫。請從 notes.previous.json 回復備份。",
                )?;
                if board.upgrade_categories()? {
                    let backup = self.path.with_file_name(format!(
                        "notes-before-categories-v2-{}-{}.json",
                        crate::model::now(),
                        std::process::id()
                    ));
                    let mut saved = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(backup)
                        .map_err(|_| "無法備份分類更新前的便條，原檔未被改寫。")?;
                    saved
                        .write_all(&bytes)
                        .and_then(|_| saved.sync_all())
                        .map_err(|_| "分類更新前的備份未完成，原檔未被改寫。")?;
                    self.save(&board)?;
                }
                board.validate()?;
                Ok(board)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Board::default()),
            Err(_) => Err("無法讀取便條資料，請檢查資料夾權限。".into()),
        }
    }
    pub fn save(&self, board: &Board) -> Result<(), String> {
        board.validate()?;
        let bytes = serde_json::to_vec_pretty(board).map_err(|_| "便條無法轉成儲存格式。")?;
        let tmp = self.path.with_extension("tmp");
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|_| "便條尚未存好：無法寫入資料夾。")?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "便條尚未存好：磁碟寫入失敗。")?;
        drop(file);
        if self.path.exists() {
            fs::copy(&self.path, self.path.with_file_name("notes.previous.json"))
                .map_err(|_| "無法保存上一版便條，這次修改尚未寫入。")?;
        }
        replace(&tmp, &self.path).map_err(|_| "便條尚未存好：無法替換資料檔。")?;
        Ok(())
    }
    pub fn export(&self, board: &Board) -> Result<PathBuf, String> {
        let path = self
            .path
            .with_file_name(format!("notes-export-{}.json", crate::model::now()));
        let mut f = File::create(&path).map_err(|_| "無法匯出便條。")?;
        f.write_all(&serde_json::to_vec_pretty(board).map_err(|_| "無法匯出便條。")?)
            .and_then(|_| f.sync_all())
            .map_err(|_| "匯出寫入失敗。")?;
        Ok(path)
    }
}
#[cfg(not(windows))]
fn replace(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::rename(src, dst)
}
#[cfg(windows)]
fn replace(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let s: Vec<u16> = src.as_os_str().encode_wide().chain(Some(0)).collect();
    let d: Vec<u16> = dst.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe {
        MoveFileExW(
            s.as_ptr(),
            d.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "return-note-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    #[test]
    fn restart_preserves_notes_draft_and_previous_version() {
        let p = dir();
        let s = Storage::new(&p).unwrap();
        let mut b = s.load().unwrap();
        b.capture("寄出合約").unwrap();
        s.save(&b).unwrap();
        b.draft = "還沒打完".into();
        s.save(&b).unwrap();
        let re = s.load().unwrap();
        assert_eq!(re.notes[0].original, "寄出合約");
        assert_eq!(re.draft, "還沒打完");
        let old: Board =
            serde_json::from_slice(&fs::read(p.join("notes.previous.json")).unwrap()).unwrap();
        assert_eq!(old.draft, "");
        fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn order_and_sidebar_survive_restart_without_editing_notes() {
        let p = dir();
        let s = Storage::new(&p).unwrap();
        let mut b = Board::default();
        for name in ["a", "b", "c"] {
            b.capture(name).unwrap();
        }
        let content = serde_json::to_value(&b.notes).unwrap();
        b.reorder_visible(&[3, 2, 1], 1, Some(3)).unwrap();
        b.settings.sidebar_collapsed = true;
        s.save(&b).unwrap();
        let loaded = s.load().unwrap();
        assert_eq!(loaded.ordered_ids(), vec![1, 3, 2]);
        assert!(loaded.settings.sidebar_collapsed);
        assert_eq!(serde_json::to_value(&loaded.notes).unwrap(), content);
        fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn category_migration_backs_up_exact_bytes_and_preserves_note_content() {
        let p = dir();
        let s = Storage::new(&p).unwrap();
        let mut b = Board::default();
        b.capture("一個星期的文稿").unwrap();
        b.capture("客戶 的待辦事項").unwrap();
        b.capture("委託案架構圖").unwrap();
        b.version = 1;
        b.notes[0].category = "未分類".into();
        b.notes[1].category = "未分類".into();
        b.notes[2].category = "產品開發".into();
        b.draft = "未送出的草稿".into();
        let bytes = serde_json::to_vec_pretty(&b).unwrap();
        fs::write(&s.path, &bytes).unwrap();
        let upgraded = s.load().unwrap();
        assert_eq!(upgraded.version, 2);
        assert_eq!(upgraded.notes[0].category, "行銷／GTM");
        assert_eq!(upgraded.notes[1].category, "外部案子");
        assert_eq!(upgraded.notes[2].category, "外部案子");
        for (before, after) in b.notes.iter().zip(&upgraded.notes) {
            let mut expected = before.clone();
            expected.category.clone_from(&after.category);
            assert_eq!(
                serde_json::to_value(expected).unwrap(),
                serde_json::to_value(after).unwrap()
            );
        }
        assert_eq!(upgraded.draft, b.draft);
        assert_eq!(fs::read(p.join("notes.previous.json")).unwrap(), bytes);
        let backups: Vec<_> = fs::read_dir(&p)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("notes-before-categories-v2-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read(backups[0].path()).unwrap(), bytes);
        let first_save = fs::read(&s.path).unwrap();
        s.load().unwrap();
        assert_eq!(fs::read(&s.path).unwrap(), first_save);
        fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn corrupt_and_future_files_are_never_silently_reset() {
        let p = dir();
        let s = Storage::new(&p).unwrap();
        fs::write(&s.path, b"broken").unwrap();
        assert!(s.load().is_err());
        assert_eq!(fs::read(&s.path).unwrap(), b"broken");
        fs::write(&s.path, b"{}").unwrap();
        assert!(s.load().is_err());
        let b = Board {
            version: 99,
            ..Board::default()
        };
        fs::write(&s.path, serde_json::to_vec(&b).unwrap()).unwrap();
        assert!(s.load().is_err());
        fs::remove_dir_all(p).unwrap();
    }
}
