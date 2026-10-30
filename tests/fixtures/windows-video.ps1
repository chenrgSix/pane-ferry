param([int]$Seconds=120, [int]$Width=1600, [int]$Height=900)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Windows.Forms,System.Drawing
Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing @'
using System;
using System.Drawing;
using System.Windows.Forms;
public class PaneFerryVideoFixture : Form {
    System.Threading.Timer timer;
    int paintPending;
    readonly System.Diagnostics.Stopwatch clock = System.Diagnostics.Stopwatch.StartNew();
    int tick, clicks, paints;
    long lastReport;
    string report=Environment.GetEnvironmentVariable("PANEFERRY_VIDEO_FIXTURE_STATS");
    [System.Runtime.InteropServices.DllImport("winmm.dll")] static extern uint timeBeginPeriod(uint ms);
    [System.Runtime.InteropServices.DllImport("winmm.dll")] static extern uint timeEndPeriod(uint ms);
    [System.Runtime.InteropServices.DllImport("user32.dll")] static extern bool ShowWindow(IntPtr hwnd,int command);
    public PaneFerryVideoFixture(int width, int height, int seconds) {
        Text="PaneFerry dynamic video verification";
        ClientSize=new Size(width,height); StartPosition=FormStartPosition.Manual; Location=new Point(60,60);
        timeBeginPeriod(1);
        DoubleBuffered=true;
        Shown += (s,e) => BeginInvoke(new Action(()=>ShowWindow(Handle,5)));
        Shown += (s,e) => {
            timer=new System.Threading.Timer(_=> {
                if(IsDisposed || System.Threading.Interlocked.Exchange(ref paintPending,1)!=0)return;
                try { BeginInvoke(new Action(()=> { ++tick; Invalidate(); if(clock.Elapsed.TotalSeconds>=seconds)Close(); })); }
                catch(InvalidOperationException) { }
            },null,0,4);
        };
    }
    protected override void OnMouseDown(MouseEventArgs e) { ++clicks; Invalidate(); base.OnMouseDown(e); }
    protected override void OnPaint(PaintEventArgs e) {
        System.Threading.Interlocked.Exchange(ref paintPending,0);
        ++paints;
        if(report!=null && clock.ElapsedMilliseconds-lastReport>=1000) {
            lastReport=clock.ElapsedMilliseconds;
            System.IO.File.WriteAllText(report,"ticks="+tick+" paints="+paints+" elapsed_ms="+lastReport);
        }
        e.Graphics.Clear(Color.White);
        for(int y=80;y<ClientSize.Height;y+=24)
            for(int x=-(tick%40);x<ClientSize.Width;x+=40)
                e.Graphics.FillRectangle(Brushes.Black,x,y,20,12);
        e.Graphics.FillRectangle(clicks%2==0?Brushes.Blue:Brushes.Green,0,0,ClientSize.Width,60);
        e.Graphics.DrawString("Frame "+tick+" / clicks "+clicks,Font,Brushes.White,16,16);
        // A changing binary marker allows capture tests to count distinct pixels.
        for(int b=0;b<16;b++) e.Graphics.FillRectangle((tick&(1<<b))!=0?Brushes.Red:Brushes.Yellow,300+b*16,8,12,36);
    }
    protected override void Dispose(bool disposing) { if(disposing){if(timer!=null)timer.Dispose();timeEndPeriod(1);} base.Dispose(disposing); }
}
'@
$form=New-Object PaneFerryVideoFixture($Width,$Height,$Seconds)
try { [System.Windows.Forms.Application]::Run($form) } finally { $form.Dispose() }
