param([int]$Seconds=45)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Windows.Forms,System.Drawing
Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing @'
using System;
using System.Drawing;
using System.Windows.Forms;
public class PaneFerryComposedMotion : Form {
    readonly Timer timer = new Timer();
    readonly Form child = new Form();
    readonly System.Diagnostics.Stopwatch clock = System.Diagnostics.Stopwatch.StartNew();
    readonly int seconds;
    int tick;
    [System.Runtime.InteropServices.DllImport("user32.dll")] static extern bool ShowWindow(IntPtr hwnd,int command);
    public PaneFerryComposedMotion(int seconds) {
        this.seconds=seconds;
        Text="PaneFerry composed motion verification";
        ClientSize=new Size(960,640); Location=new Point(50,50);
        StartPosition=FormStartPosition.Manual; DoubleBuffered=true;
        child.Text="PaneFerry synthetic moving child";
        child.FormBorderStyle=FormBorderStyle.None;
        child.Size=new Size(180,100); child.BackColor=Color.OrangeRed;
        child.ShowInTaskbar=false; child.StartPosition=FormStartPosition.Manual;
        Shown += (s,e) => {
            ShowWindow(Handle,5);
            child.Location=PointToScreen(new Point(50,100)); child.Show(this);
            timer.Interval=16;
            timer.Tick += (a,b) => {
                ++tick;
                // Move the owned surface inside a constant canvas every ~100 ms.
                if(tick%6==0) child.Location=PointToScreen(new Point(50+(tick%240),100));
                Invalidate();
                if(clock.Elapsed.TotalSeconds>=this.seconds) Close();
            };
            timer.Start();
        };
    }
    protected override void OnPaint(PaintEventArgs e) {
        e.Graphics.Clear(Color.White);
        for(int x=-(tick%32);x<ClientSize.Width;x+=32)
            e.Graphics.FillRectangle(Brushes.Blue,x,400,16,100);
    }
    protected override void Dispose(bool disposing) {
        if(disposing) { timer.Dispose(); child.Dispose(); }
        base.Dispose(disposing);
    }
}
'@
$form=New-Object PaneFerryComposedMotion($Seconds)
try { [System.Windows.Forms.Application]::Run($form) } finally { $form.Dispose() }
