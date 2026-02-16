#!/bin/sh
# Substitute only ORION_API_TOKEN into nginx config template.
# Nginx variables ($host, $remote_addr, etc.) are left intact.
envsubst '${ORION_API_TOKEN}' < /etc/nginx/nginx.conf.template > /etc/nginx/conf.d/default.conf
exec "$@"
