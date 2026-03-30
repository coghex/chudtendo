use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use super::component::{Command, HardwareMode, MemoryCommand, ReadResult, WriteResult};

#[derive(Clone, Debug)]
pub struct Bus {
    cpu: Sender<Command>,
    ppu: Sender<Command>,
    wram: Sender<Command>,
    cartridge: Sender<Command>,
    timer: Sender<Command>,
    apu: Sender<Command>,
}

impl Bus {
    pub fn new(
        cpu: Sender<Command>,
        ppu: Sender<Command>,
        wram: Sender<Command>,
        cartridge: Sender<Command>,
        timer: Sender<Command>,
        apu: Sender<Command>,
    ) -> Self {
        Self {
            cpu,
            ppu,
            wram,
            cartridge,
            timer,
            apu,
        }
    }

    pub fn read(&self, address: u16) -> PendingRead {
        let Some(target) = self.route(address) else {
            return PendingRead::ready(ReadResult::NoData);
        };

        let (respond_to, receiver) = mpsc::channel();
        let command = Command::Memory(MemoryCommand::Read {
            address,
            respond_to,
        });

        if target.send(command).is_err() {
            return PendingRead::ready(ReadResult::NoData);
        }

        PendingRead::pending(receiver)
    }

    pub fn write(&self, address: u16, value: u8) -> PendingWrite {
        let Some(target) = self.route(address) else {
            return PendingWrite::ready(WriteResult::NoData);
        };

        let (respond_to, receiver) = mpsc::channel();
        let command = Command::Memory(MemoryCommand::Write {
            address,
            value,
            respond_to: Some(respond_to),
        });

        if target.send(command).is_err() {
            return PendingWrite::ready(WriteResult::NoData);
        }

        PendingWrite::pending(receiver)
    }

    pub fn stop_components(&self) {
        for sender in [
            &self.cpu,
            &self.ppu,
            &self.wram,
            &self.cartridge,
            &self.timer,
            &self.apu,
        ] {
            let _ = sender.send(Command::Stop);
        }
    }

    pub fn apu_sender(&self) -> Sender<Command> {
        self.apu.clone()
    }

    /// Send SaveState to each component and return one receiver per component.
    /// Order: cpu, ppu, wram, cartridge, timer, apu.
    pub fn save_state_components(
        &self,
    ) -> Vec<std::sync::mpsc::Receiver<Vec<u8>>> {
        use std::sync::mpsc;
        let mut receivers = Vec::with_capacity(6);
        for sender in [
            &self.cpu,
            &self.ppu,
            &self.wram,
            &self.cartridge,
            &self.timer,
            &self.apu,
        ] {
            let (tx, rx) = mpsc::channel();
            let _ = sender.send(Command::SaveState(tx));
            receivers.push(rx);
        }
        receivers
    }

    /// Send LoadState bytes to each component in order: cpu, ppu, wram, cartridge, timer, apu.
    pub fn load_state_components(&self, blobs: [Vec<u8>; 6]) {
        let [cpu, ppu, wram, cartridge, timer, apu] = blobs;
        let _ = self.cpu.send(Command::LoadState(cpu));
        let _ = self.ppu.send(Command::LoadState(ppu));
        let _ = self.wram.send(Command::LoadState(wram));
        let _ = self.cartridge.send(Command::LoadState(cartridge));
        let _ = self.timer.send(Command::LoadState(timer));
        let _ = self.apu.send(Command::LoadState(apu));
    }

    pub fn request_ppu_features(&self) -> std::sync::mpsc::Receiver<super::component::PpuFeatures> {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = self.ppu.send(Command::RequestPpuFeatures(tx));
        rx
    }

    pub fn propagate_hardware_mode(&self, hardware_mode: HardwareMode) {
        for sender in [&self.ppu, &self.wram, &self.timer, &self.cartridge, &self.apu] {
            let _ = sender.send(Command::SetHardwareMode(hardware_mode));
        }
    }

    fn route(&self, address: u16) -> Option<&Sender<Command>> {
        match address {
            0x0000..=0x7fff | 0xa000..=0xbfff => Some(&self.cartridge),
            0x8000..=0x9fff
            | 0xfe00..=0xfe9f
            | 0xff40..=0xff4b
            | 0xff4f
            | 0xff51..=0xff55
            | 0xff68..=0xff6b => Some(&self.ppu),
            0xc000..=0xfdff | 0xff70 => Some(&self.wram),
            0xff04..=0xff07 => Some(&self.timer),
            0xff10..=0xff26 | 0xff30..=0xff3f | 0xff76..=0xff77 => Some(&self.apu),
            0xff50 => Some(&self.cartridge),
            0xfea0..=0xfeff => None,
            0xff00..=0xff7f | 0xff80..=0xffff => Some(&self.cpu),
        }
    }
}

#[derive(Debug)]
pub struct PendingRead {
    state: PendingState<ReadResult>,
}

impl PendingRead {
    fn ready(result: ReadResult) -> Self {
        Self {
            state: PendingState::Ready(Some(result)),
        }
    }

    fn pending(receiver: Receiver<ReadResult>) -> Self {
        Self {
            state: PendingState::Pending(receiver),
        }
    }

    pub fn try_take(&mut self) -> Option<ReadResult> {
        self.state.try_take(ReadResult::NoData)
    }
}

#[derive(Debug)]
pub struct PendingWrite {
    state: PendingState<WriteResult>,
}

impl PendingWrite {
    fn ready(result: WriteResult) -> Self {
        Self {
            state: PendingState::Ready(Some(result)),
        }
    }

    fn pending(receiver: Receiver<WriteResult>) -> Self {
        Self {
            state: PendingState::Pending(receiver),
        }
    }

    pub fn try_take(&mut self) -> Option<WriteResult> {
        self.state.try_take(WriteResult::NoData)
    }
}

#[derive(Debug)]
enum PendingState<T> {
    Ready(Option<T>),
    Pending(Receiver<T>),
}

impl<T: Copy> PendingState<T> {
    fn try_take(&mut self, disconnected: T) -> Option<T> {
        match self {
            Self::Ready(result) => result.take(),
            Self::Pending(receiver) => match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(disconnected),
            },
        }
    }
}
