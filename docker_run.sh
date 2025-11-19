docker run -d \
  --env-file ./.env \
  --name=connect4-moderator-server \
  --restart unless-stopped \
  -e ADMIN_AUTH="${ADMIN_AUTH}" \
  -p 5102:8080 \
  joshuafhiggins/connect4-moderator-server
