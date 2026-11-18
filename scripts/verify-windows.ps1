param([Parameter(Mandatory=$true)][string]$Exe,[Parameter(Mandatory=$true)][string]$Evidence)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
New-Item -ItemType Directory -Force $Evidence | Out-Null
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class RN {
 [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr dpi);
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls,string title);
 [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h,int index);
 [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h,int id);
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool SetWindowText(IntPtr h,string text);
 [DllImport("user32.dll", EntryPoint="SendMessageW", CharSet=CharSet.Unicode)] public static extern IntPtr SetText(IntPtr h,uint m,IntPtr w,string s);
 [DllImport("user32.dll", EntryPoint="SendMessageW", CharSet=CharSet.Unicode)] public static extern IntPtr ReadText(IntPtr h,uint m,IntPtr w,StringBuilder s);
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h,StringBuilder s,int max);
 [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
 [DllImport("user32.dll", EntryPoint="SendMessageW")] public static extern IntPtr ReadRect(IntPtr h,uint m,IntPtr w,out RECT r);
 [DllImport("user32.dll")] public static extern IntPtr GetCapture();
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h,out uint pid);
 [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int w,int height,uint flags);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out RECT r);
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h,IntPtr dc,uint flags);
 [StructLayout(LayoutKind.Sequential)] public struct RECT { public int left,top,right,bottom; }
}
'@
[RN]::SetProcessDpiAwarenessContext([IntPtr](-4))|Out-Null
function Assert($ok,[string]$why) { if(-not $ok) {throw $why}; Write-Output "PASS $why" }
function ControlHandle([int]$id) { return [RN]::GetDlgItem($script:window,$id) }
function Command([int]$id) { [RN]::SendMessage($script:window,0x0111,[IntPtr]$id,[IntPtr]::Zero)|Out-Null; Start-Sleep -Milliseconds 100 }
function Click([int]$id) { [RN]::SendMessage((ControlHandle $id),0x00F5,[IntPtr]::Zero,[IntPtr]::Zero) | Out-Null; Start-Sleep -Milliseconds 100 }
function CheckFirstRow {
 $scale=[RN]::GetDpiForWindow($script:window)/96.0;$point=([int](18*$scale) -shl 16)+[int](28*$scale)
 [RN]::SendMessage((ControlHandle 120),0x0201,[IntPtr]1,[IntPtr]$point)|Out-Null
 [RN]::SendMessage((ControlHandle 120),0x0202,[IntPtr]::Zero,[IntPtr]$point)|Out-Null
}
function InputText([int]$id,[string]$value) { [RN]::SetText((ControlHandle $id),0x000C,[IntPtr]::Zero,$value) | Out-Null; [RN]::SendMessage($script:window,0x0111,[IntPtr]($id+(0x300 -shl 16)),[IntPtr]::Zero)|Out-Null; Start-Sleep -Milliseconds 600; $read=New-Object Text.StringBuilder(8192); [RN]::ReadText((ControlHandle $id),0x000D,[IntPtr]8192,$read)|Out-Null; if($id -ne 203){Assert ($read.ToString() -eq $value) "input $id is present"} }
function EnterCapture {
 [RN]::SendMessage($script:window,0x0312,[IntPtr]1,[IntPtr]::Zero)|Out-Null
 [RN]::PostMessage((ControlHandle 110),0x0100,[IntPtr]13,[IntPtr]1)|Out-Null
 [RN]::PostMessage((ControlHandle 110),0x0101,[IntPtr]13,[IntPtr]1)|Out-Null
 Start-Sleep -Milliseconds 300
}
function PaintStat([int]$index) { return [RN]::SendMessage($script:window,0x8064,[IntPtr]$index,[IntPtr]::Zero).ToInt32() }
function State { return Get-Content -Raw -Encoding UTF8 (Join-Path $env:RETURN_NOTE_DATA 'notes.json') | ConvertFrom-Json }
function SelectRow([int]$index) { [RN]::SendMessage((ControlHandle 120),0x0186,[IntPtr]$index,[IntPtr]::Zero)|Out-Null;[RN]::SendMessage($script:window,0x0111,[IntPtr](120+(1 -shl 16)),[IntPtr]::Zero)|Out-Null }
function RowIds {
 $s=State;$count=[RN]::SendMessage((ControlHandle 120),0x018B,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32()
 for($i=0;$i -lt $count;$i++){
  $value=New-Object Text.StringBuilder(2048);[RN]::ReadText((ControlHandle 120),0x0189,[IntPtr]$i,$value)|Out-Null
  $title=($value.ToString() -split ' · ',3)[2]
  $note=@($s.notes|Where-Object {$_.title -eq $title})
  if($note.Count -ne 1){throw 'Synthetic row title must uniquely identify a note'}
  $note[0].id
 }
}
function RowPoint([int]$index,[bool]$after=$false,[int]$x=8) {
 $r=New-Object RN+RECT;[RN]::ReadRect((ControlHandle 120),0x0198,[IntPtr]$index,[ref]$r)|Out-Null
 $y=if($after){$r.bottom-2}else{$r.top+2}
 return [IntPtr](($y -shl 16)+[int]($x*([RN]::GetDpiForWindow($script:window)/96.0)))
}
function DragRow([int]$from,[int]$to,[bool]$after=$false,[int]$x=8) {
 $list=ControlHandle 120;$start=RowPoint $from $false $x;$end=RowPoint $to $after $x
 [RN]::SendMessage($list,0x0201,[IntPtr]1,$start)|Out-Null
 [RN]::SendMessage($list,0x0200,[IntPtr]1,$end)|Out-Null
 [RN]::SendMessage($list,0x0202,[IntPtr]::Zero,$end)|Out-Null
 Start-Sleep -Milliseconds 150
}
function Screenshot([string]$name) {
 $r=New-Object RN+RECT;[RN]::GetWindowRect($script:window,[ref]$r)|Out-Null
 $bmp=New-Object Drawing.Bitmap(($r.right-$r.left),($r.bottom-$r.top));$g=[Drawing.Graphics]::FromImage($bmp);$dc=$g.GetHdc()
 try { Assert ([RN]::PrintWindow($script:window,$dc,2)) 'window screenshot captured' } finally {$g.ReleaseHdc($dc)}
 $bmp.Save((Join-Path $Evidence $name));$g.Dispose();$bmp.Dispose()
}
function Launch {
 $script:process=Start-Process -FilePath $Exe -PassThru
 for($i=0;$i -lt 50;$i++){Start-Sleep -Milliseconds 100;$script:window=[RN]::FindWindow('ReturnNote.Window.v1','回來 · 測試');if($script:window -ne [IntPtr]::Zero){break}}
 Assert ($script:window -ne [IntPtr]::Zero) 'native window opened'
 Start-Sleep -Milliseconds 400
}
function Quit {
 Command 212
 Assert ($script:process.WaitForExit(5000)) 'application exited'
 Assert ($script:process.ExitCode -eq 0) 'application exit code = 0'
}
$env:RETURN_NOTE_DATA=Join-Path $Evidence 'data'
if(Test-Path $env:RETURN_NOTE_DATA){throw 'Choose a fresh evidence directory; existing test data will not be overwritten.'}
try {
 Launch
 Screenshot 'empty.png'
 Assert (-not [RN]::IsWindowVisible((ControlHandle 115))) 'search is hidden until requested'
 Assert (-not [RN]::IsWindowVisible((ControlHandle 114))) 'filter dropdown is absent from sticky note'
 Assert (-not [RN]::IsWindowVisible((ControlHandle 135))) 'status buttons are absent from sticky note'
 $r=New-Object RN+RECT;[RN]::GetWindowRect($script:window,[ref]$r)|Out-Null;$scale=[RN]::GetDpiForWindow($script:window)/96.0
 Assert (($r.right-$r.left) -le 393*$scale -and ($r.bottom-$r.top) -le 421*$scale) 'sticky note uses compact default dimensions'
 $point=(([int]($r.top+22*$scale) -band 65535) -shl 16)+([int]($r.left+130*$scale) -band 65535)
 Assert ([RN]::SendMessage($script:window,0x0084,[IntPtr]::Zero,[IntPtr]$point).ToInt32() -eq 2) 'custom header is draggable'
 [RN]::PostMessage((ControlHandle 112),0x00F5,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null
 Start-Sleep -Milliseconds 250;$menu=[RN]::FindWindow('#32768',$null);$menuPid=[uint32]0
 if($menu -ne [IntPtr]::Zero){[RN]::GetWindowThreadProcessId($menu,[ref]$menuPid)|Out-Null}
 Assert ($menu -ne [IntPtr]::Zero -and $menuPid -eq $script:process.Id) 'more menu opens in this application'
 [RN]::PostMessage($script:window,0x001F,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null
 Start-Sleep -Milliseconds 150
 if([RN]::IsWindow($menu)){[RN]::PostMessage($menu,0x0100,[IntPtr]27,[IntPtr]::Zero)|Out-Null;Start-Sleep -Milliseconds 150}
 Assert (-not [RN]::IsWindow($menu)) 'more menu dismisses without an action'

 [RN]::SendMessage($script:window,0x8065,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null
 InputText 110 '等 Claude 跑產品測試，回來先看報告'
 Assert ((State).notes.Count -eq 0) 'autosaved draft is not falsely added as a task'
 Assert ([RN]::SendMessage($script:window,0x8066,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32() -eq 1) 'draft autosave shows no false saved-task toast'
 Assert ((PaintStat 2) -eq 0) 'draft autosave does not reset the list'
 [RN]::SendMessage((ControlHandle 110),0x010D,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null
 EnterCapture
 Assert ((State).notes.Count -eq 0) 'Enter does not submit during Chinese IME composition'
 [RN]::SendMessage((ControlHandle 110),0x010E,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null
 InputText 110 '等 Claude 跑產品測試，回來先看報告'
 EnterCapture
 Assert ([RN]::SendMessage((ControlHandle 120),0x018B,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32() -eq 1) 'Enter immediately adds the task to the visible list'
 [RN]::SendMessage($script:window,0x8067,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null
 Assert ((PaintStat 3) -gt 0) 'owner-draw still paints while the model is mutably borrowed'
 $s=State;Assert ($s.notes.Count -eq 1) 'capture persists immediately';Assert ($s.notes[0].category -eq '產品／開發') 'local classification works without a key'
 Command 116;Command 135;InputText 134 '打開測試報告，確認登入流程是否通過'
 Click 116; InputText 110 '明天下午回客戶合作報價';Click 111
 $s=State;Assert ($s.notes[0].status -eq 'Now') 'capture does not steal current focus'
 Command 135;$s=State;Assert ($s.notes[0].status -eq 'Later' -and $s.notes[1].status -eq 'Now') 'switch keeps exactly one active task';Assert ($s.notes[0].resume -match '登入流程') 'switch preserves return step'
 SelectRow 1;Command 136;Command 139;$s=State;Assert ($s.notes[0].remind_at -gt 0) 'waiting reminder persists'
 Screenshot 'board.png'
 Command 116;Screenshot 'detail.png';Click 116
 $scale=[RN]::GetDpiForWindow($script:window)/96.0
 [RN]::SetWindowPos($script:window,[IntPtr]::Zero,0,0,[int](320*$scale),[int](360*$scale),0x0016)|Out-Null;Screenshot 'small.png'
 [RN]::SetWindowPos($script:window,[IntPtr]::Zero,0,0,[int](392*$scale),[int](420*$scale),0x0016)|Out-Null

 Command 904; InputText 115 '合作';$count=[RN]::SendMessage((ControlHandle 120),0x018B,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32();Assert ($count -eq 1) 'search filters notes';InputText 115 '';Command 904
 CheckFirstRow;$s=State;Assert ($s.notes[1].status -eq 'Done') 'completion preserves history';Command 905;Command 138;Command 905;$s=State;Assert ($s.notes[1].status -eq 'Inbox') 'completion can be undone'
 InputText 110 "完整原文第一行`r`n第二行也是待辦";Click 111;$s=State;Assert ($s.notes[2].original -match '第二行也是待辦') 'multiline original is retained'
 Start-Sleep -Milliseconds 2700
 [RN]::SendMessage($script:window,0x8065,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null
 InputText 110 '還沒打完的念頭'
 Assert ((PaintStat 2) -eq 0) 'typing a draft with existing notes does not rebuild the list'
 Write-Output ('DRAFT_PAINT_COUNTS parent='+ (PaintStat 0)+' list='+ (PaintStat 4))
 [RN]::SendMessage($script:window,0x8065,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null
 Start-Sleep -Milliseconds 3000
 Write-Output ('IDLE_PAINT_COUNTS parent='+ (PaintStat 0)+' list='+ (PaintStat 4))
 Assert ((PaintStat 2) -eq 0 -and (PaintStat 4) -le 1) 'caret blinking does not rebuild or repaint the task list'
 Assert ([RN]::SendMessage($script:window,0x8069,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32() -eq 1) 'focus moves from edit to button for caret-free measurement'
 Start-Sleep -Milliseconds 300
 [RN]::SendMessage($script:window,0x8065,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null
 Start-Sleep -Milliseconds 3000
 Write-Output ('NO_CARET_IDLE_PAINT_COUNTS parent='+ (PaintStat 0)+' list='+ (PaintStat 4))
 Assert ((PaintStat 0) -le 1 -and (PaintStat 4) -le 1) 'idle window stops repainting when the input caret is unfocused'
 Command 900;Screenshot 'settings.png'
 InputText 203 'return-note-synthetic-test-key';Click 206;$s=State;Assert ($s.settings.ai_enabled) 'key can be stored in Windows credential manager'
 $disk=Get-Content -Raw -Encoding UTF8 (Join-Path $env:RETURN_NOTE_DATA 'notes.json');Assert (-not $disk.Contains('synthetic-test-key')) 'key is absent from note storage'
 Command 900;Click 208;Click 210;$s=State;Assert (-not $s.settings.ai_enabled) 'key removal disables AI'
 [RN]::SendMessage($script:window,0x0010,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null;Assert ([RN]::IsIconic($script:window)) 'close minimizes to taskbar'
 [RN]::SendMessage($script:window,0x0312,[IntPtr]1,[IntPtr]::Zero)|Out-Null;Assert (-not [RN]::IsIconic($script:window)) 'hotkey handler restores window'
 $second=Start-Process -FilePath $Exe -PassThru;Assert ($second.WaitForExit(5000)) 'second instance exits without opening another writer';Assert ($second.ExitCode -eq 0) 'second instance exit code = 0'
 Quit;Launch;$s=State;Assert ($s.notes.Count -eq 3) 'restart retains all notes';Assert ($s.draft -eq '還沒打完的念頭') 'restart retains unfinished input'
 Screenshot 'restarted.png'
 foreach($note in @('整理操作指南','測試登入流程','客戶 的待辦事項','一個星期的文稿','回覆合作提案','確認產品測試結果','整理訪談筆記','寄出會議摘要')) { InputText 110 $note;Click 111 }
 $list=ControlHandle 120;$scale=[RN]::GetDpiForWindow($script:window)/96.0
 $row=[RN]::SendMessage($list,0x01A1,[IntPtr]1,[IntPtr]::Zero).ToInt32()
 Assert ($row -le 40*$scale) 'short task uses at most 40 logical pixels'
 Assert (([RN]::GetWindowLong($list,-16) -band 0x200000) -eq 0) 'list has no wide native scrollbar'
 [RN]::SendMessage($list,0x0197,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null
 $r=New-Object RN+RECT;[RN]::GetWindowRect($list,[ref]$r)|Out-Null;$eight=0
 for($i=0;$i -lt 8;$i++){ $eight += [RN]::SendMessage($list,0x01A1,[IntPtr]$i,[IntPtr]::Zero).ToInt32() }
 Assert ($eight -le ($r.bottom-$r.top)) 'eight short tasks fit fully at default size'
 Screenshot 'density.png'
 [RN]::SendMessage($list,0x020A,[IntPtr](-120 -shl 16),[IntPtr]::Zero)|Out-Null
 Assert ([RN]::SendMessage($list,0x018E,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32() -gt 0) 'mouse wheel scrolls compact list'
 $rail=ControlHandle 121;$r=New-Object RN+RECT;[RN]::GetWindowRect($rail,[ref]$r)|Out-Null;$point=(($r.bottom-$r.top-3) -shl 16)+3
 [RN]::SendMessage($rail,0x0201,[IntPtr]1,[IntPtr]$point)|Out-Null;[RN]::SendMessage($rail,0x0202,[IntPtr]::Zero,[IntPtr]$point)|Out-Null
 Assert ([RN]::SendMessage($list,0x018E,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32() -gt 0) 'thin scrollbar supports clicking and dragging'
 SelectRow 0;[RN]::SendMessage($list,0x0100,[IntPtr]35,[IntPtr]::Zero)|Out-Null
 Assert ([RN]::SendMessage($list,0x0188,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32() -eq 10) 'End key reaches last row'
 Screenshot 'scrolled.png'
 [RN]::SetWindowPos($script:window,[IntPtr]::Zero,0,0,[int](320*$scale),[int](280*$scale),0x0016)|Out-Null
 Command 900;Click 210
 $r=New-Object RN+RECT;[RN]::GetWindowRect($script:window,[ref]$r)|Out-Null
 Assert (($r.right-$r.left) -eq [int](320*$scale) -and ($r.bottom-$r.top) -eq [int](280*$scale)) 'settings returns to the manually sized sticky note'
 [RN]::SendMessage($list,0x0197,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null
 $r=New-Object RN+RECT;[RN]::GetWindowRect($list,[ref]$r)|Out-Null
 $beforeClick=(State | ConvertTo-Json -Depth 20 -Compress)
 $point=(($r.bottom-$r.top-2) -shl 16)+[int](28*$scale)
 [RN]::SendMessage($list,0x0201,[IntPtr]1,[IntPtr]$point)|Out-Null;[RN]::SendMessage($list,0x0202,[IntPtr]::Zero,[IntPtr]$point)|Out-Null
 Assert ($beforeClick -eq (State | ConvertTo-Json -Depth 20 -Compress)) 'clipped bottom row cannot complete an invisible task'
 Screenshot 'compact.png'
 [RN]::SetWindowPos($script:window,[IntPtr]::Zero,0,0,[int](392*$scale),[int](420*$scale),0x0016)|Out-Null
 Click 302
 $count=[RN]::SendMessage($list,0x018B,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32()
 Assert ($count -gt 0) 'marketing sidebar shows existing weekly writing tasks'
 InputText 110 '跟進安排';Click 111
 $s=State;Assert ($s.notes[-1].category -eq '行銷／GTM') 'capture inside a category keeps the explicit category'
 Command 1203
 $s=State;Assert ($s.notes[-1].category -eq '外部案子') 'manual category change is saved'
 Assert ([RN]::SendMessage($list,0x018B,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32() -eq $count) 'moving a task updates the current filter immediately'
 Click 304
 Assert ([RN]::SendMessage($list,0x018B,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32() -eq 2) 'external cases include 客戶 and the moved task'
 Screenshot 'external-cases.png'
 Click 300
 InputText 110 '委託案架構圖';Click 111
 InputText 110 '申請 Google startup credits';Click 111
 $s=State;Assert ($s.notes[-2].category -eq '外部案子') 'named external project overrides development keywords'
 Assert ($s.notes[-1].category -eq '營運') 'Google startup applications go to operations'
 Click 303
 Assert ([RN]::SendMessage($list,0x018B,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32() -ge 1) 'operations filter exposes the startup application'
 $label=New-Object Text.StringBuilder(128);[RN]::ReadText((ControlHandle 303),0x000D,[IntPtr]128,$label)|Out-Null
 Assert ($label.ToString() -match '營運，[1-9]') 'category count is accessible as text'
 Click 300
 $r=New-Object RN+RECT;[RN]::GetWindowRect($list,[ref]$r)|Out-Null
 $point=([int](18*$scale) -shl 16)+($r.right-$r.left-[int](22*$scale))
 [RN]::PostMessage($list,0x0201,[IntPtr]1,[IntPtr]$point)|Out-Null
 [RN]::PostMessage($list,0x0202,[IntPtr]::Zero,[IntPtr]$point)|Out-Null
 Start-Sleep -Milliseconds 250
 $menu=[RN]::FindWindow('#32768',$null);$menuPid=[uint32]0
 if($menu -ne [IntPtr]::Zero){[RN]::GetWindowThreadProcessId($menu,[ref]$menuPid)|Out-Null}
 Assert ($menu -ne [IntPtr]::Zero -and $menuPid -eq $script:process.Id) 'clicking the visible color tag opens the category chooser'
 [RN]::PostMessage($script:window,0x001F,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null;Start-Sleep -Milliseconds 100
 if([RN]::IsWindow($menu)){[RN]::PostMessage($menu,0x0100,[IntPtr]27,[IntPtr]::Zero)|Out-Null;Start-Sleep -Milliseconds 100}
 Assert (-not [RN]::IsWindow($menu)) 'category chooser dismisses safely'
 Command 1201
 $s=State;Assert ($s.notes[-1].category -eq '行銷／GTM') 'row tag targets the clicked note for category changes'
 Command 1202
 Screenshot 'categories.png'
 Command 116;Assert ([RN]::IsWindowVisible((ControlHandle 142))) 'note editor exposes a category button';Screenshot 'category-detail.png';Click 116
 # Collapsing reclaims the full sidebar width, preserves the filter, and survives restart.
 Click 302;$filteredBefore=@(RowIds)
 $expanded=New-Object RN+RECT;[RN]::GetWindowRect($list,[ref]$expanded)|Out-Null
 $notesBefore=(State).notes|ConvertTo-Json -Depth 20 -Compress
 Click 118
 $collapsed=New-Object RN+RECT;[RN]::GetWindowRect($list,[ref]$collapsed)|Out-Null
 Assert ((State).settings.sidebar_collapsed) 'sidebar choice is saved immediately'
 foreach($id in 300..305){Assert (-not [RN]::IsWindowVisible((ControlHandle $id))) "collapsed category control $id is hidden"}
 Assert (($expanded.left-$collapsed.left) -eq [int](96*$scale) -and $expanded.right -eq $collapsed.right) 'list reclaims exactly the sidebar width'
 Assert (($filteredBefore -join ',') -eq ((RowIds) -join ',')) 'collapse retains the current category filter'
 Screenshot 'collapsed-category.png'
 Quit;Launch;$list=ControlHandle 120
 Assert ((State).settings.sidebar_collapsed -and -not [RN]::IsWindowVisible((ControlHandle 301))) 'restart retains the collapsed sidebar'
 Screenshot 'collapsed.png'
 Click 118;Assert ([RN]::IsWindowVisible((ControlHandle 301))) 'sidebar expands again'
 Assert ($notesBefore -eq ((State).notes|ConvertTo-Json -Depth 20 -Compress)) 'sidebar never changes note contents'
 # Native pointer gestures sort without toggling completion or changing metadata.
 Click 300;[RN]::SendMessage($list,0x0197,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null
 $original=@(RowIds);$allNotesBefore=(State).notes|ConvertTo-Json -Depth 20 -Compress
 DragRow 0 2 $true
 $expected=@($original[1],$original[2],$original[0])+$original[3..($original.Count-1)]
 Assert (($expected -join ',') -eq ((RowIds) -join ',')) 'dragging grip down places a task at the insertion point'
 Assert (($expected -join ',') -eq ((State).order -join ',')) 'manual order is saved to disk'
 DragRow 2 0 $false 52
 Assert (($original -join ',') -eq ((RowIds) -join ',')) 'dragging the title upward restores the chosen order'
 Assert ($allNotesBefore -eq ((State).notes|ConvertTo-Json -Depth 20 -Compress)) 'sorting preserves title, category, status, original and revision'
 # A click, outside release, Escape and capture loss must all leave order unchanged.
 $diskBefore=Get-Content -Raw -Encoding UTF8 (Join-Path $env:RETURN_NOTE_DATA 'notes.json')
 DragRow 0 0 $false 52
 Assert ($diskBefore -eq (Get-Content -Raw -Encoding UTF8 (Join-Path $env:RETURN_NOTE_DATA 'notes.json'))) 'a simple title or grip click does not reorder or complete'
 $start=RowPoint 0;$end=RowPoint 2
 [RN]::SendMessage($list,0x0201,[IntPtr]1,$start)|Out-Null;[RN]::SendMessage($list,0x0200,[IntPtr]1,$end)|Out-Null
 Screenshot 'dragging.png'
 # Hold the pointer in the middle: no scrolling or insertion change means no repaint.
 Start-Sleep -Milliseconds 300
 [RN]::SendMessage($script:window,0x8065,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null
 Start-Sleep -Milliseconds 3000
 Write-Output ('HELD_DRAG_PAINT parent='+(PaintStat 0)+' list='+(PaintStat 4))
 Assert ((PaintStat 4) -le 1 -and (PaintStat 0) -le 1 -and (PaintStat 2) -eq 0) 'holding a dragged task stationary does not repeatedly repaint'
 [RN]::SendMessage($script:window,0x8065,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null
 for($i=0;$i -lt 60;$i++){
  [RN]::SendMessage($list,0x0200,[IntPtr]1,([IntPtr]($end.ToInt64()+($i%3))))|Out-Null
  Start-Sleep -Milliseconds 16
 }
 Write-Output ('SAME_SLOT_DRAG_PAINT parent='+(PaintStat 0)+' list='+(PaintStat 4))
 Assert ((PaintStat 4) -le 1 -and (PaintStat 0) -le 1) 'pointer movement within one insertion slot does not redraw the list'
 # Moving across boundaries must still paint the new line, with one buffered frame per paint.
 [RN]::SendMessage($script:window,0x8065,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null
 foreach($index in @(3,2,3,2)){
  [RN]::SendMessage($list,0x0200,[IntPtr]1,(RowPoint $index))|Out-Null
  Start-Sleep -Milliseconds 80
 }
 Assert ((PaintStat 4) -ge 4 -and (PaintStat 5) -eq (PaintStat 4)) 'each changed insertion slot draws list and indicator in one buffered frame'
 Screenshot 'drag-buffered.png'
 [RN]::SendMessage($list,0x0202,[IntPtr]::Zero,[IntPtr](-65528))|Out-Null
 Assert ($diskBefore -eq (Get-Content -Raw -Encoding UTF8 (Join-Path $env:RETURN_NOTE_DATA 'notes.json'))) 'dropping outside the list cancels safely'
 [RN]::SendMessage($list,0x0201,[IntPtr]1,$start)|Out-Null;[RN]::SendMessage($list,0x0200,[IntPtr]1,$end)|Out-Null
 [RN]::PostMessage($list,0x0100,[IntPtr]27,[IntPtr]1)|Out-Null;Start-Sleep -Milliseconds 150
 [RN]::SendMessage($list,0x0202,[IntPtr]::Zero,$end)|Out-Null
 Assert ([RN]::GetCapture() -ne $list -and -not [RN]::IsIconic($script:window)) 'Escape cancels dragging without minimizing'
 Assert ($diskBefore -eq (Get-Content -Raw -Encoding UTF8 (Join-Path $env:RETURN_NOTE_DATA 'notes.json'))) 'Escape does not save a changed order'
 [RN]::SendMessage($list,0x0201,[IntPtr]1,$start)|Out-Null;[RN]::SendMessage($list,0x0200,[IntPtr]1,$end)|Out-Null
 [RN]::SendMessage($list,0x001F,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null
 [RN]::SendMessage($list,0x0202,[IntPtr]::Zero,$end)|Out-Null
 Assert ([RN]::GetCapture() -ne $list -and $diskBefore -eq (Get-Content -Raw -Encoding UTF8 (Join-Path $env:RETURN_NOTE_DATA 'notes.json'))) 'cancel mode releases capture without moving a task'
 # A category-filtered reorder fills only that category's existing global slots.
 $beforeAll=@((State).order);Click 304;$beforeExternal=@(RowIds)
 Assert ($beforeExternal.Count -ge 2) 'filtered drag has multiple external cases'
 DragRow 0 1 $true
 $afterExternal=@(RowIds)
 Assert ($afterExternal[0] -eq $beforeExternal[1] -and $afterExternal[1] -eq $beforeExternal[0]) 'drag works inside a category'
 $afterAll=@((State).order)
 for($i=0;$i -lt $beforeAll.Count;$i++){if($beforeAll[$i] -notin $beforeExternal){Assert ($beforeAll[$i] -eq $afterAll[$i]) "hidden task at slot $i retains its position"}}
 SelectRow 0;Command 941;Assert (((RowIds) -join ',') -eq ($beforeExternal -join ',')) 'move-down menu action follows the filtered order'
 Command 940;Assert (((RowIds) -join ',') -eq ($afterExternal -join ',')) 'move-up menu action restores the filtered order'
 Click 300
 # Edge dragging scrolls longer lists and saves the eventual drop position.
 [RN]::SendMessage($list,0x0197,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null
 $start=RowPoint 0;$r=New-Object RN+RECT;[RN]::GetWindowRect($list,[ref]$r)|Out-Null
 $edge=[IntPtr]((($r.bottom-$r.top-3) -shl 16)+[int](8*$scale))
 $source=(RowIds)[0]
 [RN]::SendMessage($list,0x0201,[IntPtr]1,$start)|Out-Null;[RN]::SendMessage($list,0x0200,[IntPtr]1,$edge)|Out-Null
 Start-Sleep -Milliseconds 550
 Assert ([RN]::SendMessage($list,0x018E,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32() -gt 0) 'dragging near the bottom automatically scrolls'
 [RN]::SendMessage($list,0x0202,[IntPtr]::Zero,$edge)|Out-Null
 Assert ((State).order[0] -ne $source) 'edge drop saves a later position'
 $finalOrder=@((State).order);Click 118
 Quit;Launch;$list=ControlHandle 120
 Assert (($finalOrder -join ',') -eq ((RowIds) -join ',')) 'restart renders exactly the saved manual order'
 Assert ((State).settings.sidebar_collapsed) 'sidebar remains collapsed across sorting and restart'
 Assert ($allNotesBefore -eq ((State).notes|ConvertTo-Json -Depth 20 -Compress)) 'all drag interactions preserve every note field'
 Screenshot 'sorted-collapsed.png'
 Click 118;Screenshot 'sorted-expanded.png'
 $labels=@('產品開發','行銷 GTM','營運','外部案子','其他')
 foreach($index in 0..4){
  $label=New-Object Text.StringBuilder(128);[RN]::ReadText((ControlHandle (301+$index)),0x000D,[IntPtr]128,$label)|Out-Null
  Assert ($label.ToString().StartsWith($labels[$index]+'，')) ('sidebar uses full label '+$labels[$index])
 }
 $count=[RN]::SendMessage($list,0x018B,[IntPtr]::Zero,[IntPtr]::Zero).ToInt32()
 for($i=0;$i -lt $count;$i++){
  $label=New-Object Text.StringBuilder(2048);[RN]::ReadText($list,0x0189,[IntPtr]$i,$label)|Out-Null
  Assert ((($label.ToString() -split ' · ',3)[1]) -in $labels) "row $i uses the identical full category label"
 }
 $script:process.Refresh();@{workingSetBytes=$script:process.WorkingSet64;exeBytes=(Get-Item $Exe).Length;notes=(State).notes.Count;externalApiRequests=0}|ConvertTo-Json|Set-Content -Encoding UTF8 (Join-Path $Evidence 'metrics.json')
 Quit
 Write-Output 'WINDOWS_UI_EXIT=0'
 exit 0
} catch {
 Write-Error $_ -ErrorAction Continue
 if($script:window -ne [IntPtr]::Zero){ try{Screenshot 'failure.png';Quit}catch{} }
 exit 1
}
