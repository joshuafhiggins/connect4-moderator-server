const WebSocket = require('ws');
const readline = require('readline');

const url = 'wss://connect4.abunchofknowitalls.com';
console.log(`Connecting to ${url}...`);
const ws = new WebSocket(url);

let pingInterval;

ws.on('open', () => {
    console.log('Connected to server!');
    console.log('Type a message and press Enter to send raw text.');
    // Keep the connection alive by sending a ping every 30 seconds
    pingInterval = setInterval(() => {
        if (ws.readyState === WebSocket.OPEN) {
            ws.ping();
        }
    }, 30000);
});

ws.on('message', (data) => {
    // data is usually a Buffer in 'ws', convert to string
    console.log('< ' + data.toString());
});

ws.on('pong', () => {
    // console.log('Received pong');
});

ws.on('close', () => {
    console.log('Disconnected from server.');
    if (pingInterval) clearInterval(pingInterval);
    process.exit(0);
});

ws.on('error', (err) => {
    console.error('WebSocket error:', err.message);
    if (pingInterval) clearInterval(pingInterval);
});

const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
    prompt: '> '
});

rl.prompt();

rl.on('line', (line) => {
    if (ws.readyState === WebSocket.OPEN) {
        ws.send(line);
    } else {
        console.log('Socket not open. State:', ws.readyState);
    }
    rl.prompt();
});
