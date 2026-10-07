$events = Get-WinEvent -LogName Application -MaxEvents 400 -ErrorAction SilentlyContinue |
    Where-Object { $_.Message -match 'winxer' -and $_.Id -eq 1000 } |
    Select-Object -First 8
foreach ($e in $events) {
    $m = $e.Message
    if ($m -match 'winxer\.exe' -and $m -match '(?m)^(.+?): (.+?)$') {}
    $faulting = if ($m -match ': ([\w\-\.\(\) ]+?\.(dll|vst3))') { $Matches[1] } else { '?' }
    $code = if ($m -match ': 0x(c0000005\w*)') { $Matches[1] } else { '?' }
    "{0} | {1} | {2}" -f $e.TimeCreated, $faulting, $code
}
