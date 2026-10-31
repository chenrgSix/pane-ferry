param([ValidateSet('420','444-atlas-v1')][string]$Color='420', [int]$Frames=120, [string]$WindowId='', [string]$WindowTitle='', [string]$StatePath=(Join-Path $env:LOCALAPPDATA 'PaneFerry'), [ValidateRange(0,10000)][int]$KeyframeEvery=0)
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
    $enable=@{type='video.enable';codec='h264';max_fps=60;feedback=$true}
    if($Color -ne '420') { if($hello.capabilities -notcontains 'video.color.444-atlas-v1'){throw 'Publisher does not support full chroma'}; $enable.color=$Color }
    Send-Packet $enable
    $ready=Read-Packet 'video.ready'
    Send-Packet @{type='app.launch';app_id=$selected.id;request_id=[guid]::NewGuid().ToString();backend='native';attach=$true}
    $snapshot=Read-Packet 'window.created'
    $times=@(); $bytes=0
    $streams=[Collections.Generic.HashSet[UInt64]]::new()
    $revisions=[Collections.Generic.HashSet[UInt64]]::new()
    $clock=[Diagnostics.Stopwatch]::StartNew()
    for($i=0;$i -lt $Frames;$i++) {
        if($KeyframeEvery -gt 0 -and $i -gt 0 -and $i % $KeyframeEvery -eq 0){Send-Packet @{type='video.keyframe';window_id=1}}
        $frame=Read-Packet 'frame'
        [void]$streams.Add([UInt64]$frame.stream_id)
        [void]$revisions.Add([UInt64]$frame.geometry_revision)
        $times+= $clock.Elapsed.TotalSeconds
        $bytes+= $frame.data.Length*3/4
        # Match the Mac receiver's approximately 50 ms acknowledgement cadence.
        if($i % 3 -eq 2){Send-Packet @{type='video.feedback';frame_id=$frame.frame_id}}
    }
    $intervals=@(for($j=1;$j -lt $times.Count;$j++){1000*($times[$j]-$times[$j-1])}) | Sort-Object
    [pscustomobject]@{
        advertised_max_fps=$hello.video_max_fps
        negotiated_fps=$ready.max_fps
        color=$frame.color
        encoded_width=$frame.encoded_width
        encoded_height=$frame.encoded_height
        width=$snapshot.width
        height=$snapshot.height
        frames=$Frames
        interval_p50_ms=[Math]::Round($intervals[[int][Math]::Floor(($intervals.Count-1)*0.50)],2)
        interval_p95_ms=[Math]::Round($intervals[[int][Math]::Floor(($intervals.Count-1)*0.95)],2)
        interval_p99_ms=[Math]::Round($intervals[[int][Math]::Floor(($intervals.Count-1)*0.99)],2)
        interval_max_ms=[Math]::Round($intervals[-1],2)
        keyframe_every=$KeyframeEvery
        stream_count=$streams.Count
        geometry_revision_count=$revisions.Count
        local_delivered_fps=[Math]::Round(($Frames-1)/($times[-1]-$times[0]),2)
        approximate_mbps=[Math]::Round($bytes*8/$clock.Elapsed.TotalSeconds/1000000,2)
    } | ConvertTo-Json
} finally {
    if($writer){try{Send-Packet @{type='session.detach'}}catch{}; $writer.Dispose()}
    if($reader){$reader.Dispose()}
    $pipe.Dispose()
}
