// SPDX-License-Identifier: GPL-3.0-or-later
// Test glue only. FingerprintIO independently constructs, validates, parses and
// serializes records; this file contains no binary format encoder or decoder.
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import com.machinezoo.fingerprintio.TemplateFormatException;
import com.machinezoo.fingerprintio.ansi378v2004.*;
import com.machinezoo.fingerprintio.iso19794p2v2005.*;

public class FingerprintIoProbe {
    private static byte[] produceAnsi(int count) {
        Ansi378v2004Template template = new Ansi378v2004Template();
        template.width = 640;
        template.height = 480;
        template.resolutionX = 197;
        template.resolutionY = 198;
        template.sensorCertified = true;
        template.sensorId = 0x123;
        template.vendorId = 0x103;
        template.subformat = 0x456;
        for (int index = 0; index < count; ++index) {
            Ansi378v2004Fingerprint view = new Ansi378v2004Fingerprint();
            view.position = index == 1 ? Ansi378v2004Position.LEFT_INDEX : Ansi378v2004Position.RIGHT_INDEX;
            view.view = index == 2 ? 1 : 0;
            view.scanType = index == 1 ? Ansi378v2004ScanType.SWIPE : Ansi378v2004ScanType.LIVE_ROLLED;
            view.quality = 80 + index;
            int[] angles = {0, 73, 179};
            for (int i = 0; i < 3; ++i) {
                Ansi378v2004Minutia minutia = new Ansi378v2004Minutia();
                minutia.positionX = 40 + 20 * i + index;
                minutia.positionY = 60 + 21 * i + index;
                minutia.type = Ansi378v2004MinutiaType.values()[i];
                minutia.angle = angles[i];
                minutia.quality = 70 + i;
                view.minutiae.add(minutia);
            }
            template.fingerprints.add(view);
        }
        return template.toByteArray(); // Invokes FingerprintIO's own validator.
    }
    private static byte[] produceIso(int count) {
        Iso19794p2v2005Template template = new Iso19794p2v2005Template();
        template.width = 640;
        template.height = 480;
        template.resolutionX = 197;
        template.resolutionY = 198;
        template.sensorCertified = true;
        template.sensorId = 0x123;
        for (int index = 0; index < count; ++index) {
            Iso19794p2v2005Fingerprint view = new Iso19794p2v2005Fingerprint();
            view.position = index == 1 ? Iso19794p2v2005Position.LEFT_INDEX : Iso19794p2v2005Position.RIGHT_INDEX;
            view.view = index == 2 ? 1 : 0;
            view.scanType = index == 1 ? Iso19794p2v2005ScanType.SWIPE : Iso19794p2v2005ScanType.LIVE_ROLLED;
            view.quality = 80 + index;
            int[] angles = {0, 139, 255};
            for (int i = 0; i < 3; ++i) {
                Iso19794p2v2005Minutia minutia = new Iso19794p2v2005Minutia();
                minutia.positionX = 40 + 20 * i + index;
                minutia.positionY = 60 + 21 * i + index;
                minutia.type = Iso19794p2v2005MinutiaType.values()[i];
                minutia.angle = angles[i];
                minutia.quality = 70 + i;
                view.minutiae.add(minutia);
            }
            template.fingerprints.add(view);
        }
        return template.toByteArray();
    }
    private static byte[] consume(String format, byte[] bytes, int count) {
        if (format.equals("ansi")) {
            Ansi378v2004Template template = new Ansi378v2004Template(bytes);
            if (count >= 0 && (template.fingerprints.size() != count || template.width != 640
                    || template.height != 480 || template.resolutionX != 197 || template.resolutionY != 198
                    || template.sensorId != 0x123 || !template.sensorCertified
                    || template.vendorId != 0x103 || template.subformat != 0x456))
                throw new AssertionError("ANSI semantic fields disagree");
            return template.toByteArray();
        }
        Iso19794p2v2005Template template = new Iso19794p2v2005Template(bytes);
        if (count >= 0 && (template.fingerprints.size() != count || template.width != 640
                || template.height != 480 || template.resolutionX != 197 || template.resolutionY != 198
                || template.sensorId != 0x123 || !template.sensorCertified))
            throw new AssertionError("ISO semantic fields disagree");
        return template.toByteArray();
    }
    private static void reject(String format, byte[] bytes) {
        try {
            consume(format, bytes, -1);
        } catch (TemplateFormatException expected) {
            return;
        }
        throw new AssertionError("FingerprintIO accepted malformed input");
    }
    public static void main(String[] args) throws Exception {
        String mode = args[0];
        String format = args[1];
        if (!format.equals("ansi") && !format.equals("iso"))
            throw new IllegalArgumentException("unknown format");
        Path input = Path.of(args[2]);
        Path output = Path.of(args[3]);
        int count = Integer.parseInt(args[4]);
        if (mode.equals("produce")) {
            Files.write(output, format.equals("ansi") ? produceAnsi(count) : produceIso(count));
        } else if (mode.equals("consume")) {
            Files.write(output, consume(format, Files.readAllBytes(input), count));
        } else if (mode.equals("reject-truncations")) {
            byte[] bytes = Files.readAllBytes(input);
            for (int length = 0; length < bytes.length; ++length)
                reject(format, Arrays.copyOf(bytes, length));
            byte[] corrupt = bytes.clone();
            int header = format.equals("ansi") ? 26 : 24;
            corrupt[header + 4] = (byte)0xc0; // Reserved minutia type 3.
            reject(format, corrupt);
            System.out.println("TemplateFormatException: rejected " + bytes.length + " truncated prefixes and reserved minutia type");
        } else {
            throw new IllegalArgumentException("unknown mode");
        }
    }
}
