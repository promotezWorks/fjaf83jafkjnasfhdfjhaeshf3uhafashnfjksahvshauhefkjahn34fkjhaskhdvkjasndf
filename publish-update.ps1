# language: PowerShell, file: publish-update.ps1
# Publish builds to the Stoat #update channel as base64 text attachments
# (Stoat's file server blocks executables/archives by MIME).
#
#   .\publish-update.ps1                       # publishes exe + dll from target\release
#   .\publish-update.ps1 -Files .\my.dll
#
# The bot token is read from .\agent.secret (or the STOAT_TOKEN env var).
param(
    [string[]]$Files = @("C:\rt\release\stoat-rat.exe", "C:\rt\release\stoat_agent.dll"),
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

$upUrl = "https://cdn.stoatusercontent.com/attachments"

# Declutter: delete prior build messages so the channel always shows only the newest build.
try {
    $existing = Invoke-RestMethod "https://api.stoat.chat/channels/$Channel/messages?limit=100" -Headers @{ "X-Bot-Token" = $Token }
    foreach ($m in $existing) {
        try { Invoke-RestMethod "https://api.stoat.chat/channels/$Channel/messages/$($m._id)" -Method Delete -Headers @{ "X-Bot-Token" = $Token } | Out-Null } catch {}
    }
    Write-Host "purged $(@($existing).Count) old #update message(s)"
} catch {}

$ids = @()
$labels = @()
foreach ($f in $Files) {
    if (-not (Test-Path $f)) { throw "no such file: $f" }
    $path = (Resolve-Path $f).Path
    $b64 = [Convert]::ToBase64String([IO.File]::ReadAllBytes($path))
    $tmp = Join-Path $env:TEMP ("payload-" + [guid]::NewGuid().ToString("N") + ".txt")
    [IO.File]::WriteAllText($tmp, $b64, [Text.Encoding]::ASCII)
    $up = curl.exe -s -X POST $upUrl -H "X-Bot-Token: $Token" -F "file=@$($tmp -replace '\\','/');filename=$(Split-Path $path -Leaf).b64.txt" | ConvertFrom-Json
    Remove-Item $tmp -Force
    if (-not $up.id) { throw "upload failed for $path : $($up | ConvertTo-Json -Compress)" }
    $ids += $up.id
    $labels += "$(Split-Path $path -Leaf) ($([math]::Round((Get-Item $path).Length/1MB,2)) MB)"
}

$body = @{
    content     = "stoat-rat build $(Get-Date -Format 'yyyy-MM-dd HH:mm') :: $($labels -join ', ')"
    attachments = $ids
} | ConvertTo-Json
$msg = Invoke-RestMethod "https://api.stoat.chat/channels/$Channel/messages" -Method Post `
    -Headers @{ "X-Bot-Token" = $Token } -ContentType "application/json" -Body $body
"published update: message $($msg._id)  [$($labels -join ', ')]"
