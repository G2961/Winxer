$events = Get-WinEvent -FilterHashtable @{LogName='Application'; Id=1000} -MaxEvents 10
foreach ($e in $events) {
  $app = $e.Properties[0].Value
  if ($app -like '*winxer*') {
    Write-Output ("TIME: " + $e.TimeCreated)
    Write-Output $e.Message
    Write-Output "===="
  }
}
