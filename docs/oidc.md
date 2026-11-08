# Sign-in with OIDC

The gate signs each person in with your OIDC provider, such as Authgear, and
checks every token it gets. Set the provider up once, then start the gate with
its issuer and client.

## Register the client

Create one client for moor at your provider. Note its issuer URL and client
id. Then:

1. Add the redirect URI `https://HOST/auth/callback`. The account page, the
   webhook setup page, and machine pairing sign in through it.
2. Add the redirect URI `http://localhost:53682/callback`. `moor login` and the
   herdr plugin sign in through it.
3. Let the client sign in as a public client with PKCE, with the scopes
   `openid`, `email`, and `offline_access`.

The gate reads the person's email from the token. A person with the same email
in Slack and at your provider is one moor user, with one set of secrets.

## Register the MCP resource

Agents such as Claude Code reach `https://HOST/mcp` with access tokens of
their own.

1. Register `https://HOST/mcp` as an API resource.
2. Give it the scopes `moor:read` and `moor:write`.
3. Let MCP clients register themselves. The gate registers no clients.

The gate answers `https://HOST/.well-known/oauth-protected-resource` with the
issuer and the scopes, so a client finds the rest itself.

## Start the gate with them

```sh
moor-gate \
  --front-socket ~/.local/state/moor/fronts/authgear/front \
  --issuer https://ISSUER \
  --client-id CLIENT_ID \
  --mcp-resource-url https://HOST/mcp \
  --public-url https://HOST
```

- Without `--mcp-resource-url`, `/mcp` is off.
- Without `--public-url`, browser sign-in is off. The account page, webhook
  setup, and machine pairing then answer 404.
- If the gate cannot reach the issuer's discovery URL, pass `--jwks-url`.
- With more than one gate replica, give every replica the same
  `--cookie-key`, a secret of 32 or more characters.

On Kubernetes, the chart's `authgear.issuer`, `authgear.clientId`,
`mcp.resourceUrl`, and `gate.login` set the same flags.

## Check it

1. Open `https://HOST/auth/me`. It sends you through the provider and then
   shows the email you signed in as.
2. Run `moor doctor --public-url https://HOST` on the controller host. Its
   `gate` line says whether the gate answers.
3. Run `claude mcp add --transport http moor https://HOST/mcp`, then `/mcp` in
   Claude Code. It should sign in and list the tools.
