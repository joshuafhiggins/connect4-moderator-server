docker run -d \
  --env-file ./.env \
  --name=connect4-moderator-server \
  --restart unless-stopped \
  -e ADMIN_PASSWORD="${ADMIN_PASSWORD}" \
  -p 5102:8080 \
  joshuafhiggins/connect4-moderator-server
