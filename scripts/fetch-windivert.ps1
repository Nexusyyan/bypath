# Downloads and verifies the official signed WinDivert 2.2.2-A binaries (LGPLv3).
# Binaries are placed under runtime/windivert/ and gitignored so the repo stays pure source.

param(
    [string]$Version = "2.2.2-A",
    [string]$Tag = "v2.2.2",
    [string]$ExpectedSha256 = "63CB41763BB4B20F600B6DE04E991A9C2BE73279E317D4D82F237B150C5F3F15",
    [string]$OutputDir = (Join-Path $PSScriptRoot "..\runtime\windivert"),
    [string]$Proxy = ""
)

$ErrorActionPreference = "Stop"

$ZipName = "WinDivert-$Version.zip"
$Url = "https://github.com/basil00/WinDivert/releases/download/$Tag/$ZipName"
$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("bypath-windivert-" + [System.Guid]::NewGuid().ToString("N"))
$ZipPath = Join-Path $TempDir $ZipName

New-Item -ItemType Directory -Force -Path $TempDir | Out-Null
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

try {
    Write-Host "Downloading WinDivert $Version from $Url ..."
    if ($Proxy) {
        $CurlExe = Join-Path $env:SystemRoot "System32\curl.exe"
        if (Test-Path $CurlExe) {
            Write-Host "Using proxy $Proxy via curl.exe ..."
            & $CurlExe -fSL --proxy $Proxy -o $ZipPath $Url
            if ($LASTEXITCODE -ne 0) {
                throw "curl.exe failed with exit code $LASTEXITCODE"
            }
        } else {
            Invoke-WebRequest -Uri $Url -OutFile $ZipPath -Proxy $Proxy -UseBasicParsing
        }
    } else {
        Invoke-WebRequest -Uri $Url -OutFile $ZipPath -UseBasicParsing
    }

    $Hash = (Get-FileHash -Path $ZipPath -Algorithm SHA256).Hash
    Write-Host "Archive SHA-256: $Hash"
    if ($Version -eq "2.2.2-A" -and $ExpectedSha256 -and ($Hash -ne $ExpectedSha256)) {
        throw "SHA-256 mismatch! Expected $ExpectedSha256, got $Hash"
    }

    $ExtractDir = Join-Path $TempDir "extracted"
    Expand-Archive -Path $ZipPath -DestinationPath $ExtractDir -Force

    $RootDir = Join-Path $ExtractDir "WinDivert-$Version"
    if (-not (Test-Path $RootDir)) {
        $RootDir = (Get-ChildItem -Directory $ExtractDir | Select-Object -First 1).FullName
    }

    $X64Dir = Join-Path $RootDir "x64"
    $IncludeDir = Join-Path $RootDir "include"

    Copy-Item (Join-Path $X64Dir "WinDivert.dll") -Destination (Join-Path $OutputDir "WinDivert.dll") -Force
    Copy-Item (Join-Path $X64Dir "WinDivert64.sys") -Destination (Join-Path $OutputDir "WinDivert64.sys") -Force
    if (Test-Path (Join-Path $IncludeDir "windivert.h")) {
        Copy-Item (Join-Path $IncludeDir "windivert.h") -Destination (Join-Path $OutputDir "windivert.h") -Force
    }
    foreach ($LicenseFile in @("LICENSE", "README")) {
        $Src = Join-Path $RootDir $LicenseFile
        if (Test-Path $Src) {
            Copy-Item $Src -Destination (Join-Path $OutputDir "WINDIVERT_$LicenseFile.txt") -Force
        }
    }

    $SysPath = Join-Path $OutputDir "WinDivert64.sys"
    $Sig = Get-AuthenticodeSignature -FilePath $SysPath
    Write-Host "WinDivert64.sys signature status: $($Sig.Status)"
    if ($Sig.SignerCertificate) {
        Write-Host "Signer: $($Sig.SignerCertificate.Subject)"
    }
    if ($Sig.Status -ne "Valid") {
        Write-Warning "WinDivert64.sys Authenticode signature is not Valid ($($Sig.Status)). Check your system certificate store or driver integrity."
    }

    Write-Host "WinDivert $Version installed to $OutputDir"
}
finally {
    if (Test-Path $TempDir) {
        Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
    }
}
