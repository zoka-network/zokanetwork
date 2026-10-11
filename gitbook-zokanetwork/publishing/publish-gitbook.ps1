param(
    [Parameter(Mandatory=$false)]
    [string] $SpaceId,

    [Parameter(Mandatory=$false)]
    [string] $OrganizationId,

    [Parameter(Mandatory=$false)]
    [string] $SiteId,

    [Parameter(Mandatory=$false)]
    [string] $RepoUrl = "https://github.com/zoka-network/zokanetwork.git",

    [Parameter(Mandatory=$false)]
    [string] $Ref = "refs/heads/main",

    [Parameter(Mandatory=$false)]
    [string] $ProjectDirectory = "gitbook-zokanetwork",

    [Parameter(Mandatory=$false)]
    [switch] $Standalone
)

$ErrorActionPreference = "Stop"

if (-not $env:GITBOOK_TOKEN) {
    throw "Set GITBOOK_TOKEN in the environment before running this script."
}

$headers = @{
    Authorization = "Bearer $($env:GITBOOK_TOKEN)"
    "Content-Type" = "application/json"
}

if (-not $SpaceId) {
    if (-not $OrganizationId -or -not $SiteId) {
        throw "Pass either -SpaceId or both -OrganizationId and -SiteId."
    }

    $siteSpacesUri = "https://api.gitbook.com/v1/orgs/$OrganizationId/sites/$SiteId/site-spaces"
    Write-Host "Looking up spaces linked to GitBook site $SiteId..."
    $siteSpaces = Invoke-RestMethod -Method Get -Uri $siteSpacesUri -Headers $headers
    $items = @($siteSpaces.items)

    if ($items.Count -eq 0) {
        throw "No spaces are linked to site $SiteId."
    }

    $first = $items[0]
    $SpaceId = if ($first.space.id) { $first.space.id } elseif ($first.space) { $first.space } else { $first.id }

    if (-not $SpaceId) {
        throw "Could not infer a spaceId from the site-spaces API response."
    }

    Write-Host "Using linked space: $SpaceId"
}

$body = @{
    url = $RepoUrl
    ref = $Ref
    repoProjectDirectory = $ProjectDirectory
    force = $true
}

if ($Standalone.IsPresent) {
    $body.standalone = $true
}

$json = $body | ConvertTo-Json -Depth 10
$uri = "https://api.gitbook.com/v1/spaces/$SpaceId/git/import"

Write-Host "Importing $RepoUrl@$Ref/$ProjectDirectory into GitBook space $SpaceId..."
$response = Invoke-RestMethod -Method Post -Uri $uri -Headers $headers -Body $json
$response | ConvertTo-Json -Depth 20
