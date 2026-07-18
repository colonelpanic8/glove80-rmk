#!/usr/bin/env python3
"""Control Glove80 host extensions over ZMK Studio USB serial."""

from __future__ import annotations

import argparse
import dataclasses
import os
import selectors
import sys
import termios
import time
import tty
from collections.abc import Iterable


SOF = 0xAB
ESC = 0xAC
EOF = 0xAD
DEFAULT_DEVICE = "/dev/ttyACM0"


class LightingError(RuntimeError):
    pass


def varint(value: int) -> bytes:
    if value < 0:
        raise ValueError("protobuf integers must be non-negative")
    result = bytearray()
    while value >= 0x80:
        result.append((value & 0x7F) | 0x80)
        value >>= 7
    result.append(value)
    return bytes(result)


def uint_field(field: int, value: int) -> bytes:
    return varint(field << 3) + varint(value)


def message_field(field: int, value: bytes) -> bytes:
    return varint((field << 3) | 2) + varint(len(value)) + value


def studio_request(request_id: int, subsystem: int, request: bytes) -> bytes:
    return uint_field(1, request_id) + message_field(subsystem, request)


def capabilities_request(request_id: int) -> bytes:
    return studio_request(request_id, 6, uint_field(1, 1))


def clear_request(request_id: int) -> bytes:
    return studio_request(request_id, 6, uint_field(3, 1))


def enter_bootloader_request(request_id: int, target: str) -> bytes:
    return studio_request(request_id, 7, uint_field(1, 1 if target == "right" else 0))


def set_pixels_request(
    request_id: int,
    pixels: Iterable[tuple[int, int]],
    *,
    replace: bool,
    timeout_ms: int,
) -> bytes:
    body = bytearray()
    for index, rgb in pixels:
        body.extend(message_field(1, uint_field(1, index) + uint_field(2, rgb)))
    if replace:
        body.extend(uint_field(2, 1))
    if timeout_ms:
        body.extend(uint_field(3, timeout_ms))
    return studio_request(request_id, 6, message_field(2, bytes(body)))


def frame(payload: bytes) -> bytes:
    encoded = bytearray([SOF])
    for byte in payload:
        if byte in (SOF, ESC, EOF):
            encoded.append(ESC)
        encoded.append(byte)
    encoded.append(EOF)
    return bytes(encoded)


class FrameDecoder:
    def __init__(self) -> None:
        self.active = False
        self.escaped = False
        self.data = bytearray()

    def feed(self, chunk: bytes) -> list[bytes]:
        frames: list[bytes] = []
        for byte in chunk:
            if not self.active:
                if byte == SOF:
                    self.active = True
                    self.escaped = False
                    self.data.clear()
                continue
            if self.escaped:
                self.data.append(byte)
                self.escaped = False
            elif byte == ESC:
                self.escaped = True
            elif byte == EOF:
                frames.append(bytes(self.data))
                self.active = False
                self.data.clear()
            elif byte == SOF:
                self.escaped = False
                self.data.clear()
            else:
                self.data.append(byte)
        return frames


class ProtoReader:
    def __init__(self, data: bytes) -> None:
        self.data = data
        self.position = 0

    def read_varint(self) -> int:
        result = 0
        shift = 0
        while shift < 70 and self.position < len(self.data):
            byte = self.data[self.position]
            self.position += 1
            result |= (byte & 0x7F) << shift
            if not byte & 0x80:
                return result
            shift += 7
        raise LightingError("invalid protobuf varint")

    def read_bytes(self) -> bytes:
        length = self.read_varint()
        end = self.position + length
        if end > len(self.data):
            raise LightingError("truncated protobuf field")
        result = self.data[self.position:end]
        self.position = end
        return result

    def fields(self) -> Iterable[tuple[int, int, int | bytes]]:
        while self.position < len(self.data):
            tag = self.read_varint()
            field, wire_type = tag >> 3, tag & 7
            if wire_type == 0:
                yield field, wire_type, self.read_varint()
            elif wire_type == 2:
                yield field, wire_type, self.read_bytes()
            elif wire_type == 1:
                self.position += 8
            elif wire_type == 5:
                self.position += 4
            else:
                raise LightingError(f"unsupported protobuf wire type {wire_type}")


@dataclasses.dataclass(frozen=True)
class Capabilities:
    protocol_version: int
    pixel_count: int
    pixels_per_half: int
    max_updates_per_request: int
    max_update_hz: int
    default_timeout_ms: int
    max_timeout_ms: int
    max_channel_value: int
    supports_replace: bool
    supports_split: bool


def decode_capabilities(payload: bytes) -> Capabilities:
    values = {field: int(value) for field, wire, value in ProtoReader(payload).fields() if wire == 0}
    return Capabilities(
        protocol_version=values.get(1, 0),
        pixel_count=values.get(2, 0),
        pixels_per_half=values.get(3, 0),
        max_updates_per_request=values.get(4, 0),
        max_update_hz=values.get(5, 0),
        default_timeout_ms=values.get(6, 0),
        max_timeout_ms=values.get(7, 0),
        max_channel_value=values.get(8, 0),
        supports_replace=values.get(9, 0) == 1,
        supports_split=values.get(10, 0) == 1,
    )


def decode_response(payload: bytes) -> tuple[int, str, Capabilities | int] | None:
    request_response = next(
        (value for field, wire, value in ProtoReader(payload).fields() if field == 1 and wire == 2),
        None,
    )
    if not isinstance(request_response, bytes):
        return None
    request_id = 0
    subsystem_response: tuple[int, bytes] | None = None
    for field, wire, value in ProtoReader(request_response).fields():
        if field == 1 and wire == 0:
            request_id = int(value)
        elif field in (2, 6, 7) and wire == 2 and isinstance(value, bytes):
            subsystem_response = (field, value)
    if subsystem_response is None:
        return request_id, "unknown", -1
    subsystem, response = subsystem_response
    if subsystem == 2:
        for field, wire, value in ProtoReader(response).fields():
            if field == 2 and wire == 0:
                return request_id, "error", int(value)
        return request_id, "unknown", -1
    if subsystem == 7:
        for field, wire, value in ProtoReader(response).fields():
            if field == 1 and wire == 0:
                return request_id, "bootloader", int(value)
        return request_id, "unknown", -1
    for field, wire, value in ProtoReader(response).fields():
        if field == 1 and wire == 2 and isinstance(value, bytes):
            return request_id, "capabilities", decode_capabilities(value)
        if field in (2, 3) and wire == 0:
            return request_id, "set" if field == 2 else "clear", int(value)
    return request_id, "unknown", -1


class SerialClient:
    def __init__(self, device: str, response_timeout: float = 2.0) -> None:
        self.device = device
        self.response_timeout = response_timeout
        self.fd = -1
        self.request_id = 1
        self.decoder = FrameDecoder()

    def __enter__(self) -> "SerialClient":
        try:
            self.fd = os.open(self.device, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
        except PermissionError as error:
            raise LightingError(
                f"permission denied opening {self.device}; grant this login serial access "
                "(normally by adding it to the dialout group), then log out and back in"
            ) from error
        except FileNotFoundError as error:
            raise LightingError(f"serial device {self.device} does not exist") from error
        tty.setraw(self.fd)
        attributes = termios.tcgetattr(self.fd)
        attributes[4] = termios.B115200
        attributes[5] = termios.B115200
        termios.tcsetattr(self.fd, termios.TCSANOW, attributes)
        termios.tcflush(self.fd, termios.TCIOFLUSH)
        return self

    def __exit__(self, *_: object) -> None:
        if self.fd >= 0:
            os.close(self.fd)
            self.fd = -1

    def call(self, request: bytes) -> tuple[str, Capabilities | int]:
        expected_id = self.request_id
        self.request_id = (self.request_id + 1) & 0xFFFFFFFF
        outgoing = frame(request)
        while outgoing:
            try:
                written = os.write(self.fd, outgoing)
                outgoing = outgoing[written:]
            except BlockingIOError:
                time.sleep(0.005)

        deadline = time.monotonic() + self.response_timeout
        selector = selectors.DefaultSelector()
        selector.register(self.fd, selectors.EVENT_READ)
        try:
            while time.monotonic() < deadline:
                for _key, _events in selector.select(max(0, deadline - time.monotonic())):
                    try:
                        chunk = os.read(self.fd, 4096)
                    except BlockingIOError:
                        continue
                    for incoming in self.decoder.feed(chunk):
                        response = decode_response(incoming)
                        if response is None or response[0] != expected_id:
                            continue
                        return response[1], response[2]
        finally:
            selector.close()
        raise LightingError(f"keyboard did not respond to Studio request {expected_id}")

    def capabilities(self) -> Capabilities:
        kind, result = self.call(capabilities_request(self.request_id))
        if kind != "capabilities" or not isinstance(result, Capabilities):
            raise LightingError("keyboard does not expose the host-lighting protocol")
        if result.protocol_version != 1:
            raise LightingError(f"unsupported host-lighting protocol version {result.protocol_version}")
        return result

    def set_pixels(
        self,
        pixels: list[tuple[int, int]],
        *,
        replace: bool,
        timeout_ms: int,
    ) -> None:
        kind, result = self.call(
            set_pixels_request(
                self.request_id,
                pixels,
                replace=replace,
                timeout_ms=timeout_ms,
            )
        )
        if kind != "set" or result != 0:
            names = {
                1: "invalid pixel",
                2: "partial update",
                3: "right half unavailable",
                4: "internal error",
            }
            raise LightingError(f"keyboard rejected lighting update: {names.get(result, result)}")

    def clear(self) -> None:
        kind, result = self.call(clear_request(self.request_id))
        if kind != "clear" or result != 0:
            raise LightingError(f"keyboard rejected clear request: {result}")

    def enter_bootloader(self, target: str) -> None:
        kind, result = self.call(enter_bootloader_request(self.request_id, target))
        if kind != "bootloader" or result != 1:
            raise LightingError(f"keyboard rejected {target} bootloader request: {result}")


def parse_color(value: str) -> int:
    text = value.removeprefix("#").removeprefix("0x")
    if len(text) != 6:
        raise argparse.ArgumentTypeError("color must be a six-digit RGB value such as ff0066")
    try:
        return int(text, 16)
    except ValueError as error:
        raise argparse.ArgumentTypeError("color must contain only hexadecimal digits") from error


def parse_pixel(value: str) -> tuple[int, int]:
    try:
        index_text, color_text = value.split("=", 1)
        index = int(index_text)
    except ValueError as error:
        raise argparse.ArgumentTypeError("pixel must use INDEX=RRGGBB, such as 12=ff0066") from error
    if index < 0:
        raise argparse.ArgumentTypeError("pixel index must be non-negative")
    return index, parse_color(color_text)


def scale_color(rgb: int, maximum: int) -> int:
    channels = [(rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF]
    peak = max(channels)
    if not peak or peak <= maximum:
        return rgb
    scaled = [round(channel * maximum / peak) for channel in channels]
    return (scaled[0] << 16) | (scaled[1] << 8) | scaled[2]


def command_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", default=DEFAULT_DEVICE, help="Studio serial device")
    subcommands = parser.add_subparsers(dest="command", required=True)
    subcommands.add_parser("capabilities", help="show firmware lighting capabilities")

    all_parser = subcommands.add_parser("all", help="set every key to one color")
    all_parser.add_argument("color", type=parse_color, help="six-digit RGB color")
    all_parser.add_argument("--timeout-ms", type=int, default=None)
    all_parser.add_argument(
        "--batch-size",
        type=int,
        default=None,
        help="pixels per RPC (default: firmware maximum)",
    )

    set_parser = subcommands.add_parser("set", help="set one or more indexed keys")
    set_parser.add_argument("pixels", type=parse_pixel, nargs="+", metavar="INDEX=RRGGBB")
    set_parser.add_argument("--replace", action="store_true", help="clear unmentioned keys first")
    set_parser.add_argument("--timeout-ms", type=int, default=None)
    set_parser.add_argument(
        "--batch-size",
        type=int,
        default=None,
        help="pixels per RPC (default: firmware maximum)",
    )

    subcommands.add_parser("clear", help="release host control and restore firmware lighting")
    bootloader_parser = subcommands.add_parser(
        "bootloader",
        help="reboot either half into its UF2 bootloader over USB",
    )
    bootloader_parser.add_argument("target", choices=("left", "right"), nargs="?", default="left")
    return parser


def validated_timeout(argument: int | None, capabilities: Capabilities) -> int:
    value = capabilities.default_timeout_ms if argument is None else argument
    if value < 0 or value > capabilities.max_timeout_ms:
        raise LightingError(f"timeout must be between 0 and {capabilities.max_timeout_ms} ms")
    return value


def main() -> int:
    arguments = command_parser().parse_args()
    try:
        with SerialClient(arguments.device) as client:
            if arguments.command == "bootloader":
                client.enter_bootloader(arguments.target)
                print(f"{arguments.target.capitalize()} bootloader request accepted")
                return 0
            capabilities = client.capabilities()
            if arguments.command == "capabilities":
                for field in dataclasses.fields(capabilities):
                    print(f"{field.name}: {getattr(capabilities, field.name)}")
                return 0
            if arguments.command == "clear":
                client.clear()
                print("Firmware lighting restored")
                return 0

            timeout_ms = validated_timeout(arguments.timeout_ms, capabilities)
            if arguments.command == "all":
                pixels = [(index, arguments.color) for index in range(capabilities.pixel_count)]
                replace = True
            else:
                pixels = arguments.pixels
                replace = arguments.replace
            if any(index >= capabilities.pixel_count for index, _color in pixels):
                raise LightingError(f"pixel indices must be between 0 and {capabilities.pixel_count - 1}")
            pixels = [(index, scale_color(color, capabilities.max_channel_value)) for index, color in pixels]
            if capabilities.max_updates_per_request <= 0:
                raise LightingError("firmware reported an invalid update limit")
            batch_size = arguments.batch_size or capabilities.max_updates_per_request
            if batch_size <= 0 or batch_size > capabilities.max_updates_per_request:
                raise LightingError(
                    f"batch size must be between 1 and {capabilities.max_updates_per_request}"
                )
            limit = batch_size
            delay = 1 / capabilities.max_update_hz if capabilities.max_update_hz else 0
            for offset in range(0, len(pixels), limit):
                client.set_pixels(
                    pixels[offset : offset + limit],
                    replace=replace and offset == 0,
                    timeout_ms=timeout_ms,
                )
                if offset + limit < len(pixels) and delay:
                    time.sleep(delay)
            print(f"Updated {len(pixels)} key LEDs")
            return 0
    except LightingError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
