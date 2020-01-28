socat openssl-listen:9999,reuseaddr,fork,cert=cert.crt,key=cert.key,verify=0 tcp:127.0.0.1:9998
