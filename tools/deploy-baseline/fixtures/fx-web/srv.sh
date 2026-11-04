#!/bin/sh
while read -r l; do [ "$l" = "$(printf '\r')" ] && break; done
printf 'HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 3\r\nConnection: close\r\n\r\nok\n'
