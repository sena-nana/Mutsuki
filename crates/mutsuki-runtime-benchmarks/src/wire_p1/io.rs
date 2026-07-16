use std::io::{BufRead, Cursor, Read, Write};
use std::sync::mpsc::{Receiver, Sender};

pub(super) struct ChannelReader {
    receiver: Receiver<Vec<u8>>,
    current: Cursor<Vec<u8>>,
}

impl ChannelReader {
    pub(super) fn new(receiver: Receiver<Vec<u8>>) -> Self {
        Self {
            receiver,
            current: Cursor::new(Vec::new()),
        }
    }

    fn refill(&mut self) -> std::io::Result<()> {
        if self.current.position() as usize >= self.current.get_ref().len() {
            let frame = self.receiver.recv().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "response channel closed")
            })?;
            self.current = Cursor::new(frame);
        }
        Ok(())
    }
}

impl Read for ChannelReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.refill()?;
        self.current.read(buffer)
    }
}

impl BufRead for ChannelReader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.refill()?;
        self.current.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        self.current.consume(amount);
    }
}

pub(super) struct ChannelWriter {
    sender: Sender<Vec<u8>>,
    buffer: Vec<u8>,
}

impl ChannelWriter {
    pub(super) fn new(sender: Sender<Vec<u8>>) -> Self {
        Self {
            sender,
            buffer: Vec::new(),
        }
    }
}

impl Write for ChannelWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.buffer.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        self.sender
            .send(std::mem::take(&mut self.buffer))
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "request closed"))
    }
}
