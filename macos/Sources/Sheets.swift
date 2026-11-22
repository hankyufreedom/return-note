import SwiftUI

struct EditSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var model: BoardModel
    let note: Note
    @ViewState private var title: String
    @ViewState private var resume: String
    @ViewState private var category: String
    @ViewState private var error = ""

    init(model: BoardModel, note: Note) {
        self.model = model; self.note = note
        _title = State(initialValue: note.title)
        _resume = State(initialValue: note.resume)
        _category = State(initialValue: note.category)
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("編輯便條").font(.headline)
            TextField("標題", text: $title)
            Text("回來先做什麼").font(.subheadline)
            TextEditor(text: $resume).font(.body).frame(height: 80)
                .accessibilityLabel("回來先做什麼")
                .overlay(Rectangle().strokeBorder(Color.secondary.opacity(0.3)))
            Picker("標籤", selection: $category) { ForEach(model.tags) { Text($0.label).tag($0.key) } }
            if !error.isEmpty { Text(error).foregroundStyle(Color.red).font(.caption) }
            HStack {
                Spacer()
                Button("取消") { dismiss() }.keyboardShortcut(.cancelAction)
                Button("儲存") {
                    if model.perform("edit", ["id": note.id, "title": title, "resume": resume, "category": category]) != nil { dismiss() }
                    else { error = model.notice }
                }.keyboardShortcut(.defaultAction)
            }
        }.padding(18).frame(width: 360)
    }
}

struct OriginalSheet: View {
    @Environment(\.dismiss) private var dismiss
    let note: Note
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("便條原文").font(.headline)
            ScrollView { Text(note.original).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading) }
                .frame(maxHeight: 235)
            Button("關閉") { dismiss() }.keyboardShortcut(.defaultAction).frame(maxWidth: .infinity, alignment: .trailing)
        }.padding(18).frame(width: 380)
    }
}

struct TagSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var model: BoardModel
    @ViewState private var key: String?
    @ViewState private var name = ""
    @ViewState private var confirm = false
    @ViewState private var error = ""
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("管理標籤").font(.headline)
            Picker("標籤", selection: $key) {
                Text("新增標籤…").tag(nil as String?)
                ForEach(model.tags.filter { $0.key != "其他" }) { Text($0.label).tag(Optional($0.key)) }
            }
            .onChange(of: key) { value in name = value.map(model.tagName) ?? ""; error = "" }
            TextField("標籤名稱", text: $name)
            Text("可設最多 32 個，包含保留的「其他」。").font(.caption).foregroundStyle(.secondary)
            if !error.isEmpty { Text(error).font(.caption).foregroundStyle(Color.red) }
            HStack {
                Button("刪除…", role: .destructive) { confirm = true }.disabled(key == nil)
                Spacer()
                Button("關閉") { dismiss() }.keyboardShortcut(.cancelAction)
                Button(key == nil ? "新增" : "改名") { save() }.keyboardShortcut(.defaultAction)
            }
        }.padding(18).frame(width: 340)
        .confirmationDialog("要刪除這個標籤嗎？", isPresented: $confirm, titleVisibility: .visible) {
            Button("刪除標籤", role: .destructive) {
                if let key, model.perform("tag", ["key": key]) != nil { self.key = nil }
                else { error = model.notice }
            }
        } message: { Text("便條不會被刪除，會移到「其他」。") }
    }
    private func save() {
        var fields: [String: Any] = ["label": name]
        if let key { fields["key"] = key }
        if model.perform("tag", fields, notice: "已更新標籤") != nil { error = ""; if key == nil { name = "" } }
        else { error = model.notice }
    }
}

struct SettingsSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var model: BoardModel
    @ViewState private var enabled: Bool
    @ViewState private var name: String
    @ViewState private var key = ""
    @ViewState private var confirmation = false
    @ViewState private var message = ""
    @ViewState private var localError = false

    init(model: BoardModel) {
        self.model = model
        _enabled = State(initialValue: model.settings?.ai_enabled ?? false)
        _name = State(initialValue: model.settings?.model ?? "gemini-3.1-flash-lite")
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Gemini 分類設定").font(.headline)
            Toggle("用 Gemini 整理新便條", isOn: $enabled)
            Text("預設關閉，可完全離線使用。開啟後，新便條原文和標籤名稱會送到 Google；金鑰只存在 macOS 鑰匙圈，不會寫入便條或匯出檔。測試連線只傳測試文字。")
                .font(.caption).foregroundStyle(.secondary)
            TextField("模型名稱", text: $name)
            SecureField("Gemini API key（留空保留原有金鑰）", text: $key)
            if !message.isEmpty || !model.notice.isEmpty {
                Text(message.isEmpty ? model.notice : message).font(.caption)
                    .foregroundStyle((message.isEmpty ? model.failure : localError) ? Color.red : Color.secondary)
            }
            HStack {
                Button("移除金鑰…", role: .destructive) { confirmation = true }
                Spacer()
                Button("測試連線") { if save() { model.classify() } }.disabled(model.aiBusy)
                Button("關閉") { dismiss() }.keyboardShortcut(.cancelAction)
                Button("儲存") { if save() { message = "已儲存設定" } }.keyboardShortcut(.defaultAction)
            }
        }.padding(18).frame(width: 402)
            .confirmationDialog("移除 Gemini API key？", isPresented: $confirmation, titleVisibility: .visible) {
                Button("移除並關閉 Gemini", role: .destructive) {
                    do {
                        guard model.perform("preferences", ["ai_enabled": false]) != nil else { throw NoteError(model.notice) }
                        model.cancelAI(); enabled = false
                        try GeminiKey.remove(); key = ""
                        message = "已移除金鑰"; localError = false
                    } catch { message = error.localizedDescription; localError = true }
                }
            }
    }
    private func save() -> Bool {
        message = ""; localError = false
        do {
            let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
            guard name.range(of: "^[A-Za-z0-9.-]{1,100}$", options: .regularExpression) != nil else {
                throw NoteError("Gemini 模型名稱格式不正確。")
            }
            let key = key.trimmingCharacters(in: .whitespacesAndNewlines)
            if !key.isEmpty { try GeminiKey.save(key); self.key = ""; model.cancelAI() }
            guard model.perform("preferences", ["ai_enabled": enabled, "model": name], notice: "已儲存設定") != nil else {
                throw NoteError(model.notice)
            }
            if !enabled { model.cancelAI() }
            return true
        } catch { message = error.localizedDescription; localError = true; return false }
    }
}
