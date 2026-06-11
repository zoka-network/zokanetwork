# GitBook API Publishing

GitBook can import this documentation source from Git through the API.

## Required Inputs

- GitBook personal access token.
- GitBook `spaceId` for the `zokanetwork` space.
- Git repository URL, normally:

```text
https://github.com/ZokaNetwork/ZokaNetwork.git
```

- Git ref:

```text
refs/heads/main
```

- Project directory:

```text
gitbook-zokanetwork
```

## API Endpoint

The GitBook OpenAPI spec exposes:

```http
POST https://api.gitbook.com/v1/spaces/{spaceId}/git/import
Authorization: Bearer <token>
Content-Type: application/json
```

Body:

```json
{
  "url": "https://github.com/ZokaNetwork/ZokaNetwork.git",
  "ref": "refs/heads/main",
  "repoProjectDirectory": "gitbook-zokanetwork",
  "force": true
}
```

Set `standalone: true` only for a dry import that should generate a revision
without replacing primary space content.

