param([Parameter(Mandatory=$true)][string]$ResultPath, [switch]$Blocker)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing @'
public class PaneFerryInputFixture : System.Windows.Forms.Form {
    protected override bool ShowWithoutActivation { get { return true; } }
    public void RecordText(System.Windows.Forms.TextBox box, string path) {
        // UTF-16 input arrives one code unit at a time; a high surrogate is
        // temporarily incomplete until the next WM_CHAR delivers its pair.
        box.TextChanged += (sender, args) => System.IO.File.WriteAllText(
            path, box.Text, new System.Text.UTF8Encoding(false, false));
    }
}
public class PaneFerryForegroundBlocker : System.Windows.Forms.Form {
    protected override bool ShowWithoutActivation { get { return true; } }
    [System.Runtime.InteropServices.DllImport("user32.dll")]
    static extern bool LockSetForegroundWindow(uint code);
    protected override void OnActivated(System.EventArgs e) {
        base.OnActivated(e);
        LockSetForegroundWindow(1);
    }
    protected override void OnMouseDown(System.Windows.Forms.MouseEventArgs e) {
        base.OnMouseDown(e);
        LockSetForegroundWindow(1);
    }
    protected override void OnFormClosed(System.Windows.Forms.FormClosedEventArgs e) {
        LockSetForegroundWindow(2);
        base.OnFormClosed(e);
    }
}
'@
if ($Blocker) {
    $cover = New-Object PaneFerryForegroundBlocker
    $cover.Text = 'PaneFerry foreground blocker verification'
    $cover.StartPosition = 'Manual'
    $cover.Location = New-Object System.Drawing.Point(120,120)
    $cover.ClientSize = New-Object System.Drawing.Size(420,240)
    $coverTimer = New-Object System.Windows.Forms.Timer
    $coverTimer.Interval = 60000
    $coverTimer.Add_Tick({ $cover.Close() })
    $coverTimer.Start()
    try { [System.Windows.Forms.Application]::Run($cover) }
    finally { $coverTimer.Dispose(); $cover.Dispose() }
    return
}
$form = New-Object PaneFerryInputFixture
$form.Text = 'PaneFerry input verification'
$form.StartPosition = 'Manual'
$form.Location = New-Object System.Drawing.Point(120,120)
$form.ClientSize = New-Object System.Drawing.Size(420,240)
$inputBox = New-Object System.Windows.Forms.TextBox
$inputBox.Multiline = $true
$inputBox.Location = New-Object System.Drawing.Point(10,40)
$inputBox.Size = New-Object System.Drawing.Size(390,160)
$form.RecordText($inputBox, $ResultPath)
$form.Controls.Add($inputBox)
$helper = New-Object PaneFerryInputFixture
$helper.Text = 'PaneFerry noninteractive helper verification'
$helper.FormBorderStyle = 'FixedToolWindow'
$helper.ShowInTaskbar = $false
$helper.StartPosition = 'Manual'
$helper.Location = New-Object System.Drawing.Point(20,20)
$helper.Size = New-Object System.Drawing.Size(16,16)
$helper.Show()
$timeout = New-Object System.Windows.Forms.Timer
$timeout.Interval = 60000
$timeout.Add_Tick({ $form.Close() })
$timeout.Start()
try { [System.Windows.Forms.Application]::Run($form) }
finally { $timeout.Dispose(); $helper.Dispose(); $form.Dispose() }
