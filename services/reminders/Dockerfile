FROM php:8.3-fpm-alpine
RUN apk add --no-cache nginx
COPY nginx.conf /etc/nginx/nginx.conf
COPY public/ /var/www/html/
VOLUME ["/data"]
EXPOSE 3004
CMD ["sh", "-c", "php-fpm -D && nginx -g 'daemon off;'"]
