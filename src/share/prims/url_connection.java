package jvm25.url;

import java.io.IOException;
import java.io.InputStream;
import java.net.URL;
import java.net.URLConnection;
import java.util.List;
import java.util.Map;

/**
 * Minimal URLConnection implementation backed by host-side resource lookup.
 * The native methods open0/length0 are dispatched by the VM.
 */
public final class UrlConnection extends URLConnection {
    public UrlConnection(URL url) {
        super(url);
    }

    @Override
    public void connect() {
    }

    @Override
    public InputStream getInputStream() throws IOException {
        return open0();
    }

    private native InputStream open0() throws IOException;

    private native long length0();

    @Override
    public int getContentLength() {
        long length = length0();
        if (length < 0 || length > 2147483647L) {
            return -1;
        }
        return (int) length;
    }

    @Override
    public long getContentLengthLong() {
        return length0();
    }

    @Override
    public String getContentType() {
        return null;
    }

    @Override
    public String getHeaderField(String name) {
        return null;
    }

    @Override
    public Map<String, List<String>> getHeaderFields() {
        return null;
    }
}
