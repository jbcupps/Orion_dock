# Build the deploy image (release binaries, minimal runtime).
# Usage: .\scripts\build-deploy-image.ps1 [tag]
# Default tag: orion:latest
param([string]$Tag = "orion:latest")
$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $RepoRoot
Write-Host "Building deploy image: $Tag"
docker build -f docker/Dockerfile.deploy -t $Tag .
Write-Host "Built $Tag. Run: docker run -it --rm $Tag"
Write-Host "Push: docker tag $Tag <registry>/orion:<tag>; docker push <registry>/orion:<tag>"
