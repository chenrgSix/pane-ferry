param([int]$Frames=120, [string]$WindowId='', [string]$WindowTitle='', [string]$StatePath=(Join-Path $env:LOCALAPPDATA 'PaneFerry'))
# Local RWP throughput probe: no input, saved pixels, or pairing changes.
$ErrorActionPreference='Stop'
$pipePath='\\?\' + (Join-Path $StatePath 'node\management.sock')
$hash=([BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash([Text.Encoding]::Unicode.GetBytes($pipePath)))).Replace('-','').ToLowerInvariant()
$pipe=[IO.Pipes.NamedPipeClientStream]::new('.',('paneferry-'+$hash),[IO.Pipes.PipeDirection]::InOut,[IO.Pipes.PipeOptions]::Asynchronous)
function Send-Packet($value) {
    $value.version=1
    $writer.WriteLine(($value | ConvertTo-Json -Compress -Depth 5))
}
function Read-Packet([string]$kind) {
    do {
        $read=$reader.ReadLineAsync()
        if(-not $read.Wait(10000)){throw 'Publisher response timed out'}
        if($null -eq $read.Result){throw 'Publisher closed connection'}
        $value=$read.Result | ConvertFrom-Json
        if($value.type -eq 'error'){throw $value.code}
    } while($value.type -ne $kind)
    return $value
}
try {
    $pipe.Connect(5000)
    $writer=[IO.StreamWriter]::new($pipe,[Text.UTF8Encoding]::new($false),4096,$true)
    $reader=[IO.StreamReader]::new($pipe,[Text.UTF8Encoding]::new($false),$false,65536,$true)
    $writer.AutoFlush=$true
    Send-Packet @{type='hello'}
    $hello=Read-Packet 'hello'
    Send-Packet @{type='window.list'}
    $catalog=Read-Packet 'window.list'
    $selected=$catalog.windows | Where-Object { $_.auto_connectable -and -not $_.controlled -and (-not $WindowId -or $_.id -eq $WindowId) -and (-not $WindowTitle -or $_.title -eq $WindowTitle) } | Select-Object -First 1
    if(-not $selected){throw 'No available test window'}
    Send-Packet @{type='video.enable';codec='h264';max_fps=60;feedback=$true}
    $ready=Read-Packet 'video.ready'
    Send-Packet @{type='app.launch';app_id=$selected.id;request_id=[guid]::NewGuid().ToString();backend='native';attach=$true}
    $snapshot=Read-Packet 'window.created'
    $times=@(); $bytes=0
    $clock=[Diagnostics.Stopwatch]::StartNew()
    for($i=0;$i -lt $Frames;$i++) {
        $frame=Read-Packet 'frame'
        $times+= $clock.Elapsed.TotalSeconds
        $bytes+= $frame.data.Length*3/4
        # Match the Mac receiver's approximately 50 ms acknowledgement cadence.
        if($i % 3 -eq 2){Send-Packet @{type='video.feedback';frame_id=$frame.frame_id}}
    }
    $intervals=@(for($j=1;$j -lt $times.Count;$j++){1000*($times[$j]-$times[$j-1])}) | Sort-Object
    [pscustomobject]@{
        advertised_max_fps=$hello.video_max_fps
        negotiated_fps=$ready.max_fps
        width=$snapshot.width
        height=$snapshot.height
        frames=$Frames
        interval_p50_ms=[Math]::Round($intervals[[int][Math]::Floor(($intervals.Count-1)*0.50)],2)
        interval_p95_ms=[Math]::Round($intervals[[int][Math]::Floor(($intervals.Count-1)*0.95)],2)
        interval_p99_ms=[Math]::Round($intervals[[int][Math]::Floor(($intervals.Count-1)*0.99)],2)
        local_delivered_fps=[Math]::Round(($Frames-1)/($times[-1]-$times[0]),2)
        approximate_mbps=[Math]::Round($bytes*8/$clock.Elapsed.TotalSeconds/1000000,2)
    } | ConvertTo-Json
} finally {
    if($writer){try{Send-Packet @{type='session.detach'}}catch{}; $writer.Dispose()}
    if($reader){$reader.Dispose()}
    $pipe.Dispose()
}
