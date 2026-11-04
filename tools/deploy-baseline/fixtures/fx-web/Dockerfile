FROM alpine:3.22
# Alpine's busybox ships no httpd applet; serve a fixed HTTP 200 with busybox nc.
RUN mkdir -p /www && echo ok > /www/index.html
COPY index.html /www/page.html
COPY srv.sh /srv.sh
RUN chmod +x /srv.sh && echo layer1 > /www/l1 && echo layer2 > /www/l2
EXPOSE 8080
CMD ["nc", "-lk", "-p", "8080", "-e", "/srv.sh"]
