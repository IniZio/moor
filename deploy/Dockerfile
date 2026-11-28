# syntax=docker/dockerfile:1
FROM golang:1.26.11@sha256:6e59e17a20f13c102e49d82dffab66d41cffe6c266b225d82794328aea859ab5 AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY cmd/portal ./cmd/portal
COPY internal ./internal
ARG VERSION=dev
RUN CGO_ENABLED=0 go build -trimpath -ldflags "-s -w -X main.version=${VERSION}" -o /out/portal ./cmd/portal

FROM gcr.io/distroless/static-debian13:nonroot@sha256:8ce6dd87e9823c5b48dcfb2c8604714ccb351ce59607327bcc4957448da279b5
COPY --from=build /out/portal /portal
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["/portal"]
