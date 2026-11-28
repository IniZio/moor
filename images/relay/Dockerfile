FROM node:22-alpine@sha256:1314fb30ec37592d5845e5a160d522c0e680972aa534549940b8d73bde88481a
COPY http-relay.js /usr/local/lib/http-relay.js
USER node
ENTRYPOINT ["node","/usr/local/lib/http-relay.js"]
