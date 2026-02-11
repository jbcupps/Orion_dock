# Build orion-api and frontend images for production (k8s, matches Docker Compose full stack).
# Usage: .\scripts\build-full-stack-image.ps1 [registry/prefix]
# Default: orion-api:latest, orion-frontend:latest
param([string]$Prefix = "")
$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $RepoRoot

$ApiTag = "${Prefix}orion-api:latest"
$FrontendTag = "${Prefix}orion-frontend:latest"

Write-Host "Building orion-api: $ApiTag"
docker build -f docker/Dockerfile.api -t $ApiTag .

Write-Host "Building frontend: $FrontendTag"
docker build -f frontend/Dockerfile -t $FrontendTag .

Write-Host "Built $ApiTag and $FrontendTag"
Write-Host "For k8s: ensure images are available in cluster (load into kind/minikube, or push to registry)"
