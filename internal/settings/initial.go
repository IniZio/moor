package settings

// InitialText is what the editor shows until the first save: the example document, with every secret not set.
const InitialText = `# Settings. Everyone in the organisation can read and edit this document.
# A secret shows only as set or not set. Type a new one in its place and save to replace it.
repositories: []
credentials:
  github:
    # The product mints a token per task from the App's key, limited to the task's repository.
    github_app: {app_id: 12345, private_key: not set, webhook_secret: not set}
  anthropic:
    value: not set
  staging-api:
    value: not set
rules:
  npm:
    hosts: [registry.npmjs.org]
    allow: ["GET /**"]
  anthropic:
    hosts: [api.anthropic.com]
    credential: anthropic
    inject: {header: x-api-key, form: "{secret}"}
    allow: "*"
  github-api:
    hosts: [api.github.com]
    credential: github
    inject: {header: Authorization, form: "token {secret}"}
    allow:
      - "GET /repos/{repo}/**"
      - "GET|POST /repos/{repo}/pulls"
      - "POST /graphql"
    ask:
      - "DELETE /repos/{repo}/**"
    # GraphQL mutations a task may make. Merging is refused whatever this says.
    graphql_mutations:
      - createPullRequest
      - updatePullRequest
      - markPullRequestReadyForReview
      - convertPullRequestToDraft
      - addComment
      - updateIssueComment
      - addPullRequestReview
      - submitPullRequestReview
      - requestReviews
      - addLabelsToLabelable
      - removeLabelsFromLabelable
      - createIssue
      - updateIssue
    box_env: {GH_TOKEN: placeholder, GH_REPO: "{repo}"}
  github-git:
    hosts: [github.com]
    credential: github
    inject: {basic_user: x-access-token}
    allow: ["GET|POST /{repo}.git/**"]
  staging-api:
    hosts: [staging-api.shop.example]
    credential: staging-api
    inject: {header: Authorization, form: "Bearer {secret}"}
    allow: ["GET /**", "POST /orders"]
  staging-db:
    hosts: ["*.staging.shop.example:5432"]
    allow: "*"
`
