docker run -d \
  --name=connect4-moderator-server \
  --restart unless-stopped \
  -p 5101:8080 \
  joshuafhiggins/connect4-moderator-server
