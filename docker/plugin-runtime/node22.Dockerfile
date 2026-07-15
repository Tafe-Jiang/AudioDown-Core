FROM node:22-bookworm-slim

ARG PLUGIN_PATH=test-fixtures/plugins/virtual

RUN useradd --uid 10002 --create-home --shell /usr/sbin/nologin audiodown-plugin \
    && mkdir -p /plugin /sdk /run/audiodown /tmp \
    && chown -R 10002:10002 /plugin /sdk /run/audiodown /tmp

COPY --chown=10002:10002 plugin-sdk/node/ /sdk/
COPY --chown=10002:10002 ${PLUGIN_PATH}/ /plugin/

ENV AUDIODOWN_NODE_SDK_PATH=/sdk/src/index.js \
    NODE_ENV=production

WORKDIR /plugin
USER 10002:10002

ENTRYPOINT ["node", "src/index.js"]
