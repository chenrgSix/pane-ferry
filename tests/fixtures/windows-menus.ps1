param([Parameter(Mandatory=$true)][string]$ResultPath)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing @'
using System;
using System.Drawing;
using System.Windows.Forms;
public class FixtureMenuRenderer : ToolStripProfessionalRenderer {
    protected override void OnRenderToolStripBackground(ToolStripRenderEventArgs e) {
        e.Graphics.Clear(Color.Magenta);
    }
}
public static class PaneFerryMenus {
    [System.Runtime.InteropServices.DllImport("user32.dll")]
    static extern bool ShowWindow(IntPtr window,int command);
    public static void Run(string result) {
        using(var form=new Form {Text="PaneFerry menu verification",StartPosition=FormStartPosition.Manual,
            Location=new Point(160,160),ClientSize=new Size(600,400),BackColor=Color.Blue})
        using(var menu=new ContextMenuStrip {Renderer=new FixtureMenuRenderer(),ShowImageMargin=false})
        using(var timer=new Timer {Interval=90000}) {
            menu.Items.Add("Fixture menu action",null,(s,e)=>{
                System.IO.File.WriteAllText(result,"clicked");
                var child=new Form {Text="PaneFerry new scene window",StartPosition=FormStartPosition.Manual,
                    Location=new Point(form.Right+30,form.Top),ClientSize=new Size(220,160),BackColor=Color.Red,KeyPreview=true};
                child.MouseDown += (sender,args)=>System.IO.File.WriteAllText(result,"child-clicked");
                child.Shown += (sender,args)=>child.BeginInvoke(new Action(()=>System.IO.File.WriteAllText(result+".ready","ready")));
                child.KeyDown += (sender,args)=>{if(args.KeyCode==Keys.Escape)child.Close();};
                child.Show();
            });
            menu.Items.Add("Fixture second action");
            menu.Opened += (s,e)=>System.IO.File.WriteAllText(result+".bounds",String.Format("{0},{1},{2},{3}",menu.Left,menu.Top,menu.Width,menu.Height));
            form.ContextMenuStrip=menu;
            form.Shown += (s,e)=>form.BeginInvoke(new Action(()=>ShowWindow(form.Handle,5)));
            timer.Tick += (s,e)=>form.Close(); timer.Start();
            Application.Run(form);
        }
    }
}
'@
[PaneFerryMenus]::Run($ResultPath)
