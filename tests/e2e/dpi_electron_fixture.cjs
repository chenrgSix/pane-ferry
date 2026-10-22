// Run only with a test-owned Electron runtime and temporary HOME/profile.
const {app, BrowserWindow} = require('electron');
const fs = require('node:fs');
const path = require('node:path');
const state = process.argv[2];
app.setPath('userData', path.join(path.dirname(state), path.basename(state) + '-profile'));
app.disableHardwareAcceleration();
app.whenReady().then(async () => {
  const win = new BrowserWindow({width: 700, height: 450, useContentSize: true, frame: false,
    webPreferences: {sandbox: true, nodeIntegration: false, contextIsolation: true}});
  await win.loadURL('data:text/html;charset=utf-8,' + encodeURIComponent(`<!doctype html><meta charset="utf-8">
    <style>body{font:16px sans-serif;padding:16px}button{font:inherit;height:40px}input{font:inherit}</style>
    <p>Readable text / 界面文字 0123456789</p><input value="preserved original contents"><button onclick="this.dataset.clicks=String(Number(this.dataset.clicks||0)+1)">A normal control</button>`));
  let writing = false;
  setInterval(async () => {
    if (writing || win.isDestroyed()) return;
    writing = true;
    try {
      const value = await win.webContents.executeJavaScript(`(() => {const b=document.querySelector('button'),r=b.getBoundingClientRect();return {
        width:innerWidth,height:innerHeight,scale:devicePixelRatio,button_height:r.height,
        button_x:r.x+r.width/2,button_y:r.y+r.height/2,clicks:Number(b.dataset.clicks||0),text:document.querySelector('input').value}})()`);
      fs.writeFileSync(state+'.tmp', JSON.stringify({...value,pid:process.pid}));
      fs.renameSync(state+'.tmp',state);
    } finally {writing=false;}
  },100);
});
app.on('window-all-closed', () => app.quit());
