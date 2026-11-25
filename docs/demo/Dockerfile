# Recording environment for the demo video: a clean HOME, a neutral prompt,
# Node (for `npm install -g @tokenade/cli`), Claude Code as the agent, and a
# small Python project whose test suite is noisy on purpose.
FROM ghcr.io/charmbracelet/vhs:v0.12.1
# Same uid as the recording user, so the mounted Claude Code login is readable.
ARG UID=1000
RUN apt-get update \
 && apt-get install -y --no-install-recommends nodejs npm python3-pytest git less \
 && rm -rf /var/lib/apt/lists/* \
 && npm install -g @anthropic-ai/claude-code \
 && useradd -l -m -u $UID -s /bin/bash dev \
 && chown -R dev /usr/local/lib/node_modules /usr/local/bin
COPY --chown=dev project /demo
COPY --chown=dev home/ /home/dev/
USER dev
ENV LANG=C.UTF-8 LC_ALL=C.UTF-8 HOME=/home/dev PS1='$ ' IS_DEMO=1 DISABLE_AUTOUPDATER=1 \
    NPM_CONFIG_UPDATE_NOTIFIER=false NPM_CONFIG_FUND=false NPM_CONFIG_AUDIT=false
WORKDIR /demo
RUN git init -q && git -c user.name=demo -c user.email=demo@example.com add -A \
 && git -c user.name=demo -c user.email=demo@example.com commit -qm init
