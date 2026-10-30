param([Parameter(Mandatory=$true)][string]$ResultPath, [switch]$FileDialog)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing @'
using System;
using System.Drawing;
using System.Windows.Forms;
public class PaneFerryDialogFixture : Form {
    public PaneFerryDialogFixture(string title, string result) {
        Text = title;
        StartPosition = FormStartPosition.Manual;
        Location = new Point(160,160);
        ClientSize = new Size(420,240);
        var box = new TextBox { Multiline=true, Location=new Point(20,40), Size=new Size(370,150) };
        box.TextChanged += (s,e) => System.IO.File.WriteAllText(result, box.Text, new System.Text.UTF8Encoding(false,false));
        Controls.Add(box);
        KeyPreview=true;
    }
    protected override void OnKeyDown(KeyEventArgs e) {
        if(e.KeyCode==Keys.Escape) { e.Handled=true; Close(); }
        base.OnKeyDown(e);
    }
}
public class PaneFerryPassiveDialogFixture : Form {
    protected override CreateParams CreateParams {
        get { var p=base.CreateParams; p.ExStyle |= 0x08000000; return p; }
    }
}
public static class PaneFerryDialogs {
    [System.Runtime.InteropServices.DllImport("user32.dll")]
    static extern bool ShowWindow(IntPtr window, int command);
    [System.Runtime.InteropServices.DllImport("user32.dll")]
    static extern bool SetWindowPos(IntPtr window, IntPtr after, int x, int y, int width, int height, uint flags);
    public static void Run(string result, bool fileDialog) {
        using(var main=new PaneFerryDialogFixture("PaneFerry dialog parent verification",result+".parent"))
        using(var child=new Form {Text="PaneFerry owned child verification", Size=new Size(240,160)})
        using(var tiny=new Form {Text="PaneFerry owned tiny helper", FormBorderStyle=FormBorderStyle.None, Size=new Size(16,16)})
        using(var passive=new PaneFerryPassiveDialogFixture {Text="PaneFerry passive helper", Size=new Size(240,160)})
        using(var timer=new Timer {Interval=90000}) {
            timer.Tick += (s,e) => Application.Exit();
            timer.Start();
            main.Shown += (s,e) => main.BeginInvoke(new Action(() => {
                // The harness hides PowerShell's console. Explicitly show only
                // this test form after the inherited first-show SW_HIDE.
                ShowWindow(main.Handle, 5);
                child.Show(main); tiny.Show(main); passive.Show(main);
                SetWindowPos(tiny.Handle, IntPtr.Zero, 20,20,16,16,0x14);
                if(fileDialog) {
                    using(var picker=new OpenFileDialog {Title="PaneFerry file chooser verification"}) picker.ShowDialog(main);
                } else {
                    using(var dialog=new PaneFerryDialogFixture("",result)) {
                        dialog.FormBorderStyle=FormBorderStyle.FixedToolWindow;
                        dialog.ShowInTaskbar=false;
                        dialog.ShowDialog(main);
                    }
                }
                child.Close(); tiny.Close(); passive.Close();
            }));
            Application.Run(main);
        }
    }
}
'@
[PaneFerryDialogs]::Run($ResultPath, $FileDialog.IsPresent)
