const http = require('http');
const dgram = require('dgram');
const [UP_HOST, UP_PORT] = process.env.UP.split(':');
const AUTH = 'Basic ' + Buffer.from('x:' + process.env.UP_PASS).toString('base64');
const agent = new http.Agent({ keepAlive: false });
http.createServer((req, res) => {
  const h = { ...req.headers, 'proxy-authorization': AUTH, connection: 'close' };
  delete h['proxy-connection'];
  delete h['keep-alive'];
  const up = http.request({ host: UP_HOST, port: +UP_PORT, method: req.method, path: req.url, headers: h, agent }, (r) => {
    res.writeHead(r.statusCode, r.headers);
    r.pipe(res);
  });
  up.on('error', (e) => {
    if (!res.headersSent) res.writeHead(502);
    res.end('relay: ' + e.message);
  });
  req.pipe(up);
}).on('connect', (req, sock) => {
  sock.end('HTTP/1.1 405 Use the HTTPS proxy\r\n\r\n');
}).listen(8081, '0.0.0.0', () => console.log('relay on 8081'));

const dns = dgram.createSocket('udp4');
dns.on('message', (q, r) => {
  if (q.length < 12) return;
  const a = Buffer.from(q);
  a[2] = 0x81 | (q[2] & 0x01);
  a[3] = 0x85;
  a.writeUInt16BE(0, 6);
  a.writeUInt16BE(0, 8);
  a.writeUInt16BE(0, 10);
  dns.send(a, r.port, r.address);
});
dns.bind(53, '0.0.0.0', () => console.log('refusing dns on 53'));
