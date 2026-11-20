package com.example.demo;

import java.io.*;
import java.util.*;
import java.util.function.Function;

/** A rich sample. */
public class Sample<T extends Comparable<T>> implements Runnable {
    private static final int LIMIT = 10;
    private final List<T> items = new ArrayList<>();
    private int count;

    public Sample(int count) {
        this.count = count;
        super.toString();
    }

    @Override
    public void run() {
        for (T item : items) {
            System.out.println(item);
        }
    }

    public static int sum(int[] values, int offset) {
        int total = 0; // running total
        for (int i = offset; i < values.length; i++) {
            total += values[i];
        }
        /* block comment */
        return total > LIMIT ? LIMIT : total;
    }

    public String readAll(String path) throws IOException {
        StringBuilder sb = new StringBuilder();
        try (BufferedReader reader = new BufferedReader(new FileReader(path))) {
            String line;
            while ((line = reader.readLine()) != null) {
                sb.append(line).append('\n');
            }
        } catch (FileNotFoundException | SecurityException e) {
            throw new IOException("missing: " + path, e);
        } finally {
            count++;
        }
        return sb.toString();
    }

    public <R> List<R> mapAll(Function<T, R> fn) {
        List<R> out = new ArrayList<>(items.size());
        items.forEach(x -> out.add(fn.apply(x)));
        items.stream().map(Object::toString).forEach(System.out::println);
        Runnable r = new Runnable() {
            @Override
            public void run() {
                int local = count;
                System.out.println(local);
            }
        };
        r.run();
        return out;
    }

    public String describe(Object o) {
        if (o instanceof String s && !s.isEmpty()) {
            return s.toUpperCase();
        }
        switch (count) {
            case 0:
                return "zero";
            case 1: {
                int k = count * 2;
                return "one" + k;
            }
            default:
                break;
        }
        String text = """
            hello
            world""";
        double d = 1.5e3;
        long big = 0xFFL;
        char c = 'x';
        boolean flag = true;
        outer:
        for (int a = 0; a < 3; a++) {
            for (int b = 0; b < 3; b++) {
                if (a == b) continue outer;
            }
        }
        return text + d + big + c + flag;
    }

    public abstract static class Inner {
        abstract void noBody();
        void withBody(int x) { x = x + 1; }
    }
}
